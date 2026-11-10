//! Immutable dependency-interface snapshots and the deterministic legacy comparison oracle.
//!
//! The snapshot is deliberately diagnostic data in this stage: normal checking still collects every dependency
//! declaration through the legacy importer. Later stages can build and install immutable interfaces only after this
//! oracle proves that table contents, canonical identities, ordering and visibility projections remain identical.

use std::fmt::Debug;

use super::*;

/// Schema version for the complete dependency-interface snapshot compared by the shadow oracle.
pub const MODULE_DEPENDENCY_INTERFACE_SNAPSHOT_VERSION: u32 = 1;

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
                rendered.push_str(entry);
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
    pub entries: Vec<String>,
}

impl TypeChecker {
    /// Select the immutable provider handles that form the next module's exact dependency closure.
    pub fn select_dependency_interfaces(&mut self, interfaces: Vec<Arc<ModuleDependencyInterface>>) {
        self.selected_dependency_interfaces = interfaces;
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
            map_table("declaration_nominal_scopes", &self.declaration_nominal_scopes),
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
            DependencyInterfaceTableSnapshot {
                name: "symbol_rows",
                entries: self
                    .symbols
                    .dependency_interface_symbols()
                    .map(|(id, symbol, identity)| {
                        format!("{id:08}|{}|{}", stable_debug(symbol), stable_debug(&identity))
                    })
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
        self.dependency_exports
            .insert(module_name.to_string(), Arc::clone(&exports));
        self.register_dependency_derivable_metadata(module_name, module_ast);
        self.cache_dependency_direct_member_symbols(module_name, module_ast, public_only);
        self.cache_dependency_registry_definitions(module_name, module_ast, public_only);
        self.cache_dependency_registry_consts(module_ast);
        self.cache_dependency_member_symbols(module_name, module_ast, public_only);
        self.cache_dependency_adoption_traits();
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
        let owned_identity = |identity: &CanonicalSymbolId| identity_owned_by(identity, &module_path);
        let qualified = |key: &String| key_owned_by(key, &module_path);
        Arc::new(ModuleDependencyInterface {
            module_name: module_name.to_string(),
            module_path: module_path.clone(),
            public_only,
            exports,
            member_symbols: Arc::new(
                self.dependency_member_symbols
                    .get(module_name)
                    .cloned()
                    .unwrap_or_default(),
            ),
            member_type_aliases: Arc::new(
                self.dependency_member_type_aliases
                    .get(module_name)
                    .cloned()
                    .unwrap_or_default(),
            ),
            registry_definitions: self.dependency_registry_definitions.get(module_name).cloned(),
            registry_consts: Arc::new(
                self.dependency_registry_consts
                    .iter()
                    .filter(|(identity, _)| owned_identity(identity))
                    .map(|(identity, value)| (identity.clone(), value.clone()))
                    .collect(),
            ),
            member_partial_projections: Arc::new(
                self.dependency_member_partial_projections
                    .get(module_name)
                    .cloned()
                    .unwrap_or_default(),
            ),
            direct_member_symbols: Arc::new(
                self.dependency_direct_member_symbols
                    .get(module_name)
                    .cloned()
                    .unwrap_or_default(),
            ),
            direct_member_type_aliases: Arc::new(
                self.dependency_direct_member_type_aliases
                    .get(module_name)
                    .cloned()
                    .unwrap_or_default(),
            ),
            direct_member_identities: Arc::new(direct_member_identities),
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
            member_reexports: Arc::new(
                self.dependency_member_reexports
                    .get(module_name)
                    .cloned()
                    .unwrap_or_default(),
            ),
            body_ir_pub_reexports: Arc::new(
                self.body_ir_dependency_pub_reexports
                    .get(module_name)
                    .cloned()
                    .unwrap_or_default(),
            ),
            direct_member_partial_projections: Arc::new(
                self.dependency_direct_member_partial_projections
                    .get(module_name)
                    .cloned()
                    .unwrap_or_default(),
            ),
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
            snapshot: self.dependency_interface_snapshot_for(module_name),
        })
    }

    /// Replace dependency lookup caches with the exact selected provider interfaces.
    ///
    /// The caller supplies handles in collected-module closure order. Local declarations, imports, diagnostics and
    /// body facts are not touched; only the dependency layer under those local facts is replaced.
    pub fn install_dependency_interfaces(&mut self, interfaces: &[Arc<ModuleDependencyInterface>]) {
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
        for interface in interfaces {
            let name = interface.module_name.clone();
            self.dependency_exports
                .insert(name.clone(), Arc::clone(&interface.exports));
            self.dependency_member_symbols
                .insert(name.clone(), interface.member_symbols.as_ref().clone());
            self.dependency_member_type_aliases
                .insert(name.clone(), interface.member_type_aliases.as_ref().clone());
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
                .insert(name.clone(), interface.member_partial_projections.as_ref().clone());
            self.dependency_direct_member_symbols
                .insert(name.clone(), interface.direct_member_symbols.as_ref().clone());
            self.dependency_direct_member_type_aliases
                .insert(name.clone(), interface.direct_member_type_aliases.as_ref().clone());
            self.dependency_direct_member_identities
                .insert(name.clone(), interface.direct_member_identities.as_ref().clone());
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
                .insert(name.clone(), interface.member_reexports.as_ref().clone());
            self.body_ir_dependency_pub_reexports
                .insert(name.clone(), interface.body_ir_pub_reexports.as_ref().clone());
            self.dependency_module_path_segments
                .insert(name.clone(), interface.module_path.clone());
            self.dependency_direct_member_partial_projections
                .insert(name, interface.direct_member_partial_projections.as_ref().clone());
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
        }
    }

    /// Replace the just-built legacy dependency layer with the selected immutable interfaces.
    ///
    /// Shadow comparison remains a test concern so production checking never renders the diagnostic snapshot. The
    /// legacy collection immediately before this call is retained through stage 4 and removed only after those tests
    /// prove the direct route in stage 5.
    pub(super) fn activate_selected_dependency_interfaces(&mut self) {
        if self.selected_dependency_interfaces.is_empty() {
            return;
        }
        let interfaces = std::mem::take(&mut self.selected_dependency_interfaces);
        self.install_dependency_interfaces(&interfaces);
    }

    /// Capture only the tables owned by `module_name`, excluding every other provider in the checker's closure.
    fn dependency_interface_snapshot_for(&self, module_name: &str) -> ModuleDependencyInterfaceSnapshot {
        let module_path = self
            .dependency_module_path_segments
            .get(module_name)
            .cloned()
            .unwrap_or_else(|| vec![module_name.to_string()]);
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
            filtered_map_table(
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
            DependencyInterfaceTableSnapshot {
                name: "symbol_rows",
                entries: self
                    .symbols
                    .dependency_interface_symbols()
                    .filter(|(_, _, identity)| {
                        identity
                            .map(|identity| identity_owned_by(identity, &module_path))
                            .unwrap_or(false)
                    })
                    .enumerate()
                    .map(|(index, (_, symbol, identity))| {
                        format!("{index:08}|{}|{}", stable_debug(symbol), stable_debug(&identity))
                    })
                    .collect(),
            },
        ];
        ModuleDependencyInterfaceSnapshot {
            version: MODULE_DEPENDENCY_INTERFACE_SNAPSHOT_VERSION,
            tables,
        }
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
fn module_table<V: Debug>(
    name: &'static str,
    module_name: &str,
    values: &HashMap<String, V>,
) -> DependencyInterfaceTableSnapshot {
    let entries = values
        .get(module_name)
        .map(|value| vec![format!("{module_name:?}|{}", stable_debug(value))])
        .unwrap_or_default();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Render one module's inner member map in deterministic order.
fn nested_module_table<V: Debug>(
    name: &'static str,
    module_name: &str,
    values: &HashMap<String, HashMap<String, V>>,
) -> DependencyInterfaceTableSnapshot {
    let mut entries = values
        .get(module_name)
        .into_iter()
        .flat_map(HashMap::iter)
        .map(|(member, value)| format!("{member:?}|{}", stable_debug(value)))
        .collect::<Vec<_>>();
    entries.sort();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Render one module's immutable ordered projection.
fn ordered_module_table<V: Debug>(
    name: &'static str,
    module_name: &str,
    values: &HashMap<String, Arc<[V]>>,
) -> DependencyInterfaceTableSnapshot {
    let entries = values
        .get(module_name)
        .into_iter()
        .flat_map(|members| members.iter().enumerate())
        .map(|(index, value)| format!("{index:08}|{}", stable_debug(value)))
        .collect();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Render entries selected by provider-owned canonical identity.
fn filtered_map_table<K: Debug, V: Debug>(
    name: &'static str,
    values: &impl MapEntries<K, V>,
    include: impl Fn(&K) -> bool,
) -> DependencyInterfaceTableSnapshot {
    let mut entries = values
        .entries()
        .filter(|(key, _)| include(key))
        .map(|(key, value)| format!("{}|{}", stable_debug(key), stable_debug(value)))
        .collect::<Vec<_>>();
    entries.sort();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Render module-qualified metadata whose stable key starts with the provider's canonical source path.
fn prefixed_map_table<V: Debug>(
    name: &'static str,
    module_path: &[String],
    values: &HashMap<String, V>,
) -> DependencyInterfaceTableSnapshot {
    let prefix = format!("{}.", module_path.join("."));
    let exact = module_path.join(".");
    let mut entries = values
        .iter()
        .filter(|(key, _)| *key == &exact || key.starts_with(&prefix))
        .map(|(key, value)| format!("{key:?}|{}", stable_debug(value)))
        .collect::<Vec<_>>();
    entries.sort();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Render a map into key-sorted records without relying on its randomized iteration order.
fn map_table<K: Debug, V: Debug>(
    name: &'static str,
    values: &impl MapEntries<K, V>,
) -> DependencyInterfaceTableSnapshot {
    let mut entries = values
        .entries()
        .map(|(key, value)| format!("{}|{}", stable_debug(key), stable_debug(value)))
        .collect::<Vec<_>>();
    entries.sort();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Render a module-keyed map whose inner maps also need deterministic ordering.
fn nested_table<V: Debug>(
    name: &'static str,
    values: &HashMap<String, HashMap<String, V>>,
) -> DependencyInterfaceTableSnapshot {
    let mut entries = values
        .iter()
        .flat_map(|(module, members)| {
            members
                .iter()
                .map(move |(member, value)| format!("{module:?}|{member:?}|{}", stable_debug(value)))
        })
        .collect::<Vec<_>>();
    entries.sort();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Render module-keyed immutable slices while preserving each projection's declared order.
fn ordered_nested_table<V: Debug>(
    name: &'static str,
    values: &HashMap<String, Arc<[V]>>,
) -> DependencyInterfaceTableSnapshot {
    let mut entries = values
        .iter()
        .flat_map(|(module, members)| {
            members
                .iter()
                .enumerate()
                .map(move |(index, value)| format!("{module:?}|{index:08}|{}", stable_debug(value)))
        })
        .collect::<Vec<_>>();
    entries.sort();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Render a debug value with every brace-delimited map or record sorted recursively.
///
/// Rust's derived `Debug` preserves sequence order but emits `HashMap` entries in randomized iteration order. The
/// snapshot oracle must distinguish semantic table changes from that process-local noise, including maps nested
/// inside symbol records. Sorting brace-delimited top-level fields also canonicalizes derived struct output without
/// disturbing tuple, list or source-declared sequence order.
fn stable_debug(value: &impl Debug) -> String {
    canonicalize_debug_chars(&format!("{value:?}").chars().collect::<Vec<_>>())
}

/// Canonicalize nested debug groups while preserving quoted strings and ordered delimiters.
fn canonicalize_debug_chars(chars: &[char]) -> String {
    let mut output = String::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '"' {
            let end = quoted_end(chars, index);
            output.extend(chars[index..end].iter().copied());
            index = end;
            continue;
        }
        let Some(close) = matching_delimiter(chars[index]) else {
            output.push(chars[index]);
            index += 1;
            continue;
        };
        let end = group_end(chars, index, close);
        output.push(chars[index]);
        let inner = &chars[index + 1..end - 1];
        if chars[index] == '{' {
            let mut fields = top_level_fields(inner)
                .into_iter()
                .map(|field| canonicalize_debug_chars(field).trim().to_string())
                .filter(|field| !field.is_empty())
                .collect::<Vec<_>>();
            fields.sort();
            output.push_str(&fields.join(", "));
        } else {
            output.push_str(&canonicalize_debug_chars(inner));
        }
        output.push(close);
        index = end;
    }
    output
}

/// Return the exclusive end of one quoted debug string, honoring backslash escapes.
fn quoted_end(chars: &[char], start: usize) -> usize {
    let mut index = start + 1;
    let mut escaped = false;
    while index < chars.len() {
        if !escaped && chars[index] == '"' {
            return index + 1;
        }
        escaped = !escaped && chars[index] == '\\';
        if chars[index] != '\\' {
            escaped = false;
        }
        index += 1;
    }
    chars.len()
}

/// Find the exclusive end of a balanced debug group.
fn group_end(chars: &[char], start: usize, expected_close: char) -> usize {
    let mut stack = vec![expected_close];
    let mut index = start + 1;
    while index < chars.len() {
        if chars[index] == '"' {
            index = quoted_end(chars, index);
            continue;
        }
        if let Some(close) = matching_delimiter(chars[index]) {
            stack.push(close);
        } else if stack.last().is_some_and(|close| *close == chars[index]) {
            stack.pop();
            if stack.is_empty() {
                return index + 1;
            }
        }
        index += 1;
    }
    chars.len()
}

/// Split a brace-delimited debug record at commas outside nested groups and strings.
fn top_level_fields(chars: &[char]) -> Vec<&[char]> {
    let mut fields = Vec::new();
    let mut stack = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '"' {
            index = quoted_end(chars, index);
            continue;
        }
        if let Some(close) = matching_delimiter(chars[index]) {
            stack.push(close);
        } else if stack.last().is_some_and(|close| *close == chars[index]) {
            stack.pop();
        } else if chars[index] == ',' && stack.is_empty() {
            fields.push(&chars[start..index]);
            start = index + 1;
        }
        index += 1;
    }
    fields.push(&chars[start..]);
    fields
}

/// Pair one opening debug delimiter with its close.
fn matching_delimiter(open: char) -> Option<char> {
    match open {
        '{' => Some('}'),
        '[' => Some(']'),
        '(' => Some(')'),
        _ => None,
    }
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
