//! Immutable dependency-interface snapshots and the deterministic legacy comparison oracle.
//!
//! The snapshot is deliberately diagnostic data in this stage: normal checking still collects every dependency
//! declaration through the legacy importer. Later stages can build and install immutable interfaces only after this
//! oracle proves that table contents, canonical identities, ordering and visibility projections remain identical.

use std::fmt::Debug;

use super::*;

/// Schema version for the complete dependency-interface snapshot compared by the shadow oracle.
pub const MODULE_DEPENDENCY_INTERFACE_SNAPSHOT_VERSION: u32 = 1;

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
            nested_table("member_partial_projections", &self.dependency_member_partial_projections),
            nested_table("direct_member_symbols", &self.dependency_direct_member_symbols),
            nested_table("direct_member_type_aliases", &self.dependency_direct_member_type_aliases),
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
                    .map(|(id, symbol, identity)| format!("{id:08}|{symbol:?}|{identity:?}"))
                    .collect(),
            },
        ];
        ModuleDependencyInterfaceSnapshot {
            version: MODULE_DEPENDENCY_INTERFACE_SNAPSHOT_VERSION,
            tables,
        }
    }
}

/// Render a map into key-sorted records without relying on its randomized iteration order.
fn map_table<K: Debug, V: Debug>(name: &'static str, values: &impl MapEntries<K, V>) -> DependencyInterfaceTableSnapshot {
    let mut entries = values
        .entries()
        .map(|(key, value)| format!("{key:?}|{value:?}"))
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
                .map(move |(member, value)| format!("{module:?}|{member:?}|{value:?}"))
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
                .map(move |(index, value)| format!("{module:?}|{index:08}|{value:?}"))
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
