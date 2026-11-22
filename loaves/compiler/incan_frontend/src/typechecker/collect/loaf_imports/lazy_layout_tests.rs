//! A compiled Loaf that a checker installs is installed by name: the layout of each of its declarations is built the
//! first time a lookup reads it, and the checked and sealed modules are the same as when every layout is built at once.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use super::canonical_remapping_tests::{body_ir, checker, parse};
use super::rust_reexport_chain_tests::{TestResult, installed, published_root};
use crate::loaf_index::LoafIndex;
use crate::typechecker::{CheckedNamespaceInterface, DependencyInterfacePreparationCache};

/// A Loaf whose second model nothing names.
const BASE: &str = "pub model Inner:\n    pub value: int\n\npub model Other:\n    pub count: int\n";

/// A Loaf whose `Outer` holds a `base` model, beside one declaration of each other kind that nothing names.
const SHAPES: &str = "from pub::base import Inner\n\npub model Outer:\n    pub inner: Inner\n\npub model Unused:\n    pub count: int\n\npub class Spare:\n    pub size: int\n\npub enum Mode:\n    Fast\n    Slow\n\npub type Tag = newtype str\n\npub trait Shape:\n    pass\n";

/// A project module whose signature names `Outer`, the one `shapes` type it uses.
const PROVIDER: &str = "from pub::shapes import Outer\nfrom pub::base import Inner\n\npub def make() -> Outer:\n    return Outer(inner=Inner(value=1))\n";

/// A project module that reads the provider's sealed interface and reaches `Inner` only through `Outer`'s layout.
const CONSUMER: &str = "from provider import make\n\ndef main() -> int:\n    return make().inner.value\n";

/// The installed `base` and `shapes` Loaves.
pub(super) fn shapes(root: &Path) -> TestResult<Arc<LoafIndex>> {
    let base = installed(root, &published_root("base", BASE, LoafIndex::default())?)?;
    let shapes = installed(
        root,
        &published_root("shapes", SHAPES, LoafIndex::new([base.clone()])?)?,
    )?;
    Ok(Arc::new(LoafIndex::new([base, shapes])?))
}

/// What checking the provider and the consumer publishes: the provider's sealed interface and checked namespace, the
/// consumer's dependency tables, and both modules' Body IR.
#[derive(Debug, PartialEq)]
struct CheckedGraph {
    sealed: String,
    namespace: String,
    consumer_tables: String,
    body_ir: [String; 2],
}

/// Check the provider, seal and publish it, and check the consumer against its sealed interface, every checker sharing
/// the build's preparation `cache`. Also returns the layout builds of the checks, counted before the consumer's
/// dependency tables are rendered, which builds every layout they hold.
fn check_graph(
    loaves: &Arc<LoafIndex>,
    cache: &DependencyInterfacePreparationCache,
) -> TestResult<(CheckedGraph, BTreeMap<String, usize>)> {
    let provider = parse(PROVIDER)?;
    let consumer = parse(CONSUMER)?;
    let mut first = checker("provider", loaves, cache);
    first
        .check_program(&provider)
        .map_err(|errors| format!("provider: {errors:?}"))?;
    let exports: Arc<[_]> = crate::module::exported_symbols(&provider).into();
    let interface = first.seal_current_module_dependency_interface("provider", &provider, exports, true);
    let namespace = CheckedNamespaceInterface::from_checked("provider", &["provider".to_owned()], &provider, &first)?;
    let mut second = checker("consumer", loaves, cache);
    second.select_dependency_interfaces(vec![Arc::clone(&interface)]);
    second
        .check_with_imports(&consumer, &[("provider", &provider)])
        .map_err(|errors| format!("consumer: {errors:?}"))?;
    let builds = cache.layout_build_counts();
    let consumer_tables = format!("{:?}", second.dependency_interface_snapshot());
    let checked = CheckedGraph {
        sealed: format!("{:?}", interface.snapshot()),
        namespace: format!("{namespace:?}"),
        consumer_tables,
        body_ir: [body_ir(&provider, first), body_ir(&consumer, second)],
    };
    Ok((checked, builds))
}

/// Naming one type of a compiled Loaf builds that type's layout and the layouts a lookup then reads through it, and no
/// other declaration of that Loaf or of the Loaves it names.
#[test]
fn naming_one_provider_type_builds_only_its_layout_and_the_layouts_read_through_it() -> TestResult {
    let root = tempfile::tempdir()?;
    let loaves = shapes(root.path())?;
    let (_, builds) = check_graph(&loaves, &DependencyInterfacePreparationCache::default())?;
    assert!(
        builds.contains_key("Outer") && builds.contains_key("Inner"),
        "the named layouts were not built: {builds:?}"
    );
    let unread = builds
        .keys()
        .filter(|name| !matches!(name.as_str(), "Outer" | "Inner"))
        .collect::<Vec<_>>();
    assert!(
        unread.is_empty(),
        "layouts no lookup read were built: {unread:?} of {builds:?}"
    );
    Ok(())
}

/// Building layouts on demand changes nothing that is checked or sealed: the provider's sealed interface and checked
/// namespace, the consumer's dependency tables and both modules' Body IR equal those of an install that builds every
/// layout at once.
#[test]
fn a_loaf_installed_by_name_checks_and_seals_as_one_installed_whole() -> TestResult {
    let root = tempfile::tempdir()?;
    let loaves = shapes(root.path())?;
    let (by_name, _) = check_graph(&loaves, &DependencyInterfacePreparationCache::default())?;
    let (whole, _) = check_graph(&loaves, &DependencyInterfacePreparationCache::eager_layouts())?;
    assert_eq!(by_name, whole);
    Ok(())
}

/// A Loaf's nominal type origins are a fact of the Loaf: the module graph collects them once for each Loaf, however
/// many compiled Loaves' foreign preparations reach it. Here `base` is reached by its own preparation and by that of
/// `shapes`, whose `Outer` holds a `base` model.
#[test]
fn a_loafs_type_origins_are_collected_once_per_module_graph() -> TestResult {
    let root = tempfile::tempdir()?;
    let loaves = shapes(root.path())?;
    let cache = DependencyInterfacePreparationCache::default();
    check_graph(&loaves, &cache)?;
    let collections = cache.origin_collection_counts();
    assert!(
        collections.contains_key("base") && collections.contains_key("shapes"),
        "the graph's foreign preparations reached no Loaf: {collections:?}"
    );
    assert!(
        collections.values().all(|&count| count == 1),
        "type origins collected more than once per Loaf: {collections:?}"
    );
    Ok(())
}

/// A compiled Loaf's install record holds what installing it reads of its manifest, as that manifest gives it: the
/// nominal public types with their declarations' source paths and identities, the root exports by kind and the type
/// origins. It reads back as it was written.
#[test]
fn a_loafs_install_record_holds_what_install_reads_and_reads_back() -> TestResult {
    use crate::library_manifest::CanonicalIdentityExport;
    use crate::library_manifest::install_record::LoafInstallRecord;
    use crate::typechecker::TypeChecker;
    let root = tempfile::tempdir()?;
    let loaves = shapes(root.path())?;
    for library in ["base", "shapes"] {
        let manifest = loaves.manifest(library).ok_or("missing manifest")?;
        let mut checker = TypeChecker::new();
        checker.set_loaves(Arc::clone(&loaves));
        let record = checker.loaf_install_record(library, manifest);
        let expected = checker
            .manifest_nominal_public_types(library, manifest)
            .iter()
            .map(|entry| {
                (
                    entry.public_name.clone(),
                    entry.identity.source_path.clone(),
                    entry.identity.canonical.clone(),
                )
            })
            .collect::<Vec<_>>();
        let recorded = record
            .canonical_types
            .iter()
            .map(|row| {
                (
                    row.public_name.clone(),
                    row.source_path.clone(),
                    row.canonical.as_ref().and_then(CanonicalIdentityExport::hydrate),
                )
            })
            .collect::<Vec<_>>();
        assert!(!expected.is_empty(), "{library} publishes no nominal type");
        assert_eq!(recorded, expected, "{library}'s nominal public types");
        let names = |names: Vec<&String>| names.into_iter().cloned().collect::<Vec<_>>();
        let exports = &manifest.exports;
        assert_eq!(
            record.root_exports.models,
            names(exports.models.iter().map(|item| &item.name).collect())
        );
        assert_eq!(
            record.root_exports.classes,
            names(exports.classes.iter().map(|item| &item.name).collect())
        );
        assert_eq!(
            record.root_exports.enums,
            names(exports.enums.iter().map(|item| &item.name).collect())
        );
        assert_eq!(
            record.root_exports.newtypes,
            names(exports.newtypes.iter().map(|item| &item.name).collect())
        );
        assert_eq!(
            record.root_exports.traits,
            names(exports.traits.iter().map(|item| &item.name).collect())
        );
        let origins = super::collect_nominal_type_origins(manifest)
            .into_values()
            .collect::<Vec<_>>();
        assert_eq!(record.type_origins, origins, "{library}'s type origins");
        let encoded = serde_json::to_vec(&record)?;
        assert_eq!(serde_json::from_slice::<LoafInstallRecord>(&encoded)?, record);
    }
    Ok(())
}

/// A compiled Loaf's install record lists, for each checked API module, the models, classes, enums, newtypes, type
/// aliases and traits install registers from it: each one's position, name and kind, the canonical identity the
/// identity graph publishes at its public path, a type alias's target, and the type parameters of a type alias, model
/// or class.
#[test]
fn a_loafs_install_record_lists_each_modules_nominal_declarations() -> TestResult {
    use crate::api_metadata::ApiDeclaration;
    use crate::library_manifest::CanonicalIdentityExport;
    use crate::library_manifest::install_record::InstallDeclarationKind;
    use crate::typechecker::TypeChecker;
    let root = tempfile::tempdir()?;
    let loaves = shapes(root.path())?;
    let manifest = loaves.manifest("shapes").ok_or("missing manifest")?;
    let api = manifest
        .contract_metadata
        .api
        .as_ref()
        .ok_or("shapes has no checked API")?;
    let graph = &manifest.contract_metadata.identity_graph;
    let mut checker = TypeChecker::new();
    checker.set_loaves(Arc::clone(&loaves));
    let record = checker.loaf_install_record("shapes", manifest);
    let mut expected = Vec::new();
    for module in &api.modules {
        let mut declarations = Vec::new();
        for (position, declaration) in module.declarations.iter().enumerate() {
            let (name, kind, alias_target, type_params) = match declaration {
                ApiDeclaration::Model(item) => (
                    &item.name,
                    InstallDeclarationKind::Model,
                    None,
                    item.type_params.clone(),
                ),
                ApiDeclaration::Class(item) => (
                    &item.name,
                    InstallDeclarationKind::Class,
                    None,
                    item.type_params.clone(),
                ),
                ApiDeclaration::Enum(item) => (&item.name, InstallDeclarationKind::Enum, None, Vec::new()),
                ApiDeclaration::Newtype(item) => (&item.name, InstallDeclarationKind::Newtype, None, Vec::new()),
                ApiDeclaration::Trait(item) => (&item.name, InstallDeclarationKind::Trait, None, Vec::new()),
                ApiDeclaration::TypeAlias(item) => (
                    &item.name,
                    InstallDeclarationKind::TypeAlias,
                    Some(item.type_alias.target.clone()),
                    item.type_alias.type_params.clone(),
                ),
                _ => continue,
            };
            let public_path = std::iter::once(manifest.name.clone())
                .chain(module.module_path.iter().cloned())
                .chain(std::iter::once(name.clone()))
                .collect::<Vec<_>>();
            declarations.push((
                position,
                name.clone(),
                kind,
                graph.canonical_for_public_path(&public_path),
                alias_target,
                type_params,
            ));
        }
        expected.push((module.module_path.clone(), declarations));
    }
    let recorded = record
        .modules
        .iter()
        .map(|module| {
            let declarations = module
                .declarations
                .iter()
                .map(|declaration| {
                    (
                        declaration.position,
                        declaration.name.clone(),
                        declaration.kind,
                        declaration
                            .canonical
                            .as_ref()
                            .and_then(CanonicalIdentityExport::hydrate),
                        declaration.alias_target.clone(),
                        declaration.type_params.clone(),
                    )
                })
                .collect::<Vec<_>>();
            (module.module_path.clone(), declarations)
        })
        .collect::<Vec<_>>();
    assert!(
        expected.iter().any(|(_, declarations)| declarations.len() >= 5),
        "the fixture declares too little"
    );
    assert_eq!(recorded, expected);
    Ok(())
}

/// A compiled Loaf's install record carries its identity graph exports and its root alias exports as the manifest
/// publishes them, and, for each public name the graph publishes, the kind of root export the name finds first.
#[test]
fn a_loafs_install_record_carries_its_identity_exports_and_the_kind_each_name_finds() -> TestResult {
    use super::ManifestExportRef;
    use crate::library_manifest::install_record::InstallExportKind;
    use crate::typechecker::TypeChecker;
    let root = tempfile::tempdir()?;
    let loaves = shapes(root.path())?;
    for library in ["base", "shapes"] {
        let manifest = loaves.manifest(library).ok_or("missing manifest")?;
        let mut checker = TypeChecker::new();
        checker.set_loaves(Arc::clone(&loaves));
        let record = checker.loaf_install_record(library, manifest);
        let graph = &manifest.contract_metadata.identity_graph;
        assert!(!graph.exports.is_empty(), "{library} publishes no identity");
        assert_eq!(record.identity_graph, *graph, "{library}'s identity graph");
        assert_eq!(record.aliases, manifest.exports.aliases, "{library}'s alias exports");
        let mut seen = std::collections::HashSet::new();
        let expected = graph
            .exports
            .iter()
            .filter(|export| seen.insert(export.public_name.clone()))
            .map(|export| {
                let kind = TypeChecker::find_manifest_export(manifest, &export.public_name).map(|found| match found {
                    ManifestExportRef::Alias(_) => InstallExportKind::Alias,
                    ManifestExportRef::Model(_) => InstallExportKind::Model,
                    ManifestExportRef::Class(_) => InstallExportKind::Class,
                    ManifestExportRef::Function(_) => InstallExportKind::Function,
                    ManifestExportRef::Partial(_) => InstallExportKind::Partial,
                    ManifestExportRef::Trait(_) => InstallExportKind::Trait,
                    ManifestExportRef::Enum(_) => InstallExportKind::Enum,
                    ManifestExportRef::EnumVariant { .. } => InstallExportKind::EnumVariant,
                    ManifestExportRef::TypeAlias(_) => InstallExportKind::TypeAlias,
                    ManifestExportRef::Newtype(_) => InstallExportKind::Newtype,
                    ManifestExportRef::Const(_) => InstallExportKind::Const,
                    ManifestExportRef::Static(_) => InstallExportKind::Static,
                });
                (export.public_name.clone(), kind)
            })
            .collect::<Vec<_>>();
        let recorded = record
            .export_kinds
            .iter()
            .map(|named| (named.public_name.clone(), named.kind))
            .collect::<Vec<_>>();
        assert_eq!(recorded, expected, "{library}'s export kinds");
    }
    Ok(())
}

/// Installing a compiled Loaf reads its nominal public types from its install record: given a record that lists one
/// type fewer than the manifest publishes, the Loaf's canonical type remapping binds only the types the record lists.
#[test]
fn a_loafs_canonical_type_remapping_is_the_one_its_install_record_lists() -> TestResult {
    use crate::typechecker::TypeChecker;
    let root = tempfile::tempdir()?;
    let loaves = shapes(root.path())?;
    let manifest = loaves.manifest("shapes").ok_or("missing manifest")?;
    let mut producer = TypeChecker::new();
    producer.set_loaves(Arc::clone(&loaves));
    let mut record = producer.loaf_install_record("shapes", manifest);
    assert!(record.canonical_types.iter().any(|row| row.public_name == "Unused"));
    record.canonical_types.retain(|row| row.public_name != "Unused");
    let cache = DependencyInterfacePreparationCache::default().with_install_record("shapes", record);
    let mut consumer = checker("consumer", &loaves, &cache);
    let remapping = consumer.public_library_canonical_type_remapping("shapes");
    assert!(
        remapping.contains_key("Outer"),
        "the record's types are bound: {remapping:?}"
    );
    assert!(
        !remapping.contains_key("Unused"),
        "a type the record does not list is bound: {remapping:?}"
    );
    Ok(())
}

/// Installing a compiled Loaf registers the declarations its install record lists: given a record that omits the
/// Loaf's trait, the trait keeps its root name's entry but is not registered under its declaration key.
#[test]
fn a_loaf_registers_the_declarations_its_install_record_lists() -> TestResult {
    use crate::typechecker::TypeChecker;
    let root = tempfile::tempdir()?;
    let loaves = shapes(root.path())?;
    let manifest = Arc::clone(&loaves.entry("shapes").ok_or("missing shapes")?.artifact.manifest);
    let mut producer = TypeChecker::new();
    producer.set_loaves(Arc::clone(&loaves));
    let mut record = producer.loaf_install_record("shapes", &manifest);
    let module_path = record
        .modules
        .iter()
        .find(|module| module.declarations.iter().any(|declared| declared.name == "Shape"))
        .map(|module| module.module_path.clone())
        .ok_or("the record lists no Shape")?;
    for module in &mut record.modules {
        module.declarations.retain(|declared| declared.name != "Shape");
    }
    let key = TypeChecker::declaration_type_key("shapes", &module_path, "Shape");
    let mut whole = checker("consumer", &loaves, &DependencyInterfacePreparationCache::default());
    whole.cache_transitive_pub_export_semantics("shapes");
    assert!(
        whole.transitive_pub_traits.contains_key(&key),
        "the whole record registers {key}"
    );
    let cache = DependencyInterfacePreparationCache::default().with_install_record("shapes", record);
    let mut consumer = checker("consumer", &loaves, &cache);
    consumer.cache_transitive_pub_export_semantics("shapes");
    assert!(
        consumer.transitive_pub_traits.contains_key("Shape"),
        "the root trait keeps its root name"
    );
    assert!(
        !consumer.transitive_pub_traits.contains_key(&key),
        "a declaration the record omits is registered under {key}"
    );
    Ok(())
}

/// Installing a compiled Loaf keys its root exports' layouts by the names its install record lists: given a record
/// that omits one root model, that model gets no root-name entry while the others do.
#[test]
fn a_loaf_keys_the_root_exports_its_install_record_lists() -> TestResult {
    use crate::typechecker::TypeChecker;
    let root = tempfile::tempdir()?;
    let loaves = shapes(root.path())?;
    let manifest = Arc::clone(&loaves.entry("shapes").ok_or("missing shapes")?.artifact.manifest);
    let mut producer = TypeChecker::new();
    producer.set_loaves(Arc::clone(&loaves));
    let mut record = producer.loaf_install_record("shapes", &manifest);
    assert_eq!(record.root_exports.models, ["Outer", "Unused"]);
    record.root_exports.models.retain(|name| name != "Unused");
    let cache = DependencyInterfacePreparationCache::default().with_install_record("shapes", record);
    let mut consumer = checker("consumer", &loaves, &cache);
    consumer.cache_transitive_pub_export_semantics("shapes");
    assert!(
        consumer.transitive_pub_types.contains_key("Outer"),
        "a listed root model is keyed"
    );
    assert!(
        !consumer.transitive_pub_types.contains_key("Unused"),
        "a root model the record omits is keyed by its root name"
    );
    Ok(())
}

/// Installing a compiled Loaf lays out its nominal exports under their canonical spellings from the identity graph its
/// install record carries: given a record whose graph omits one model, that model gets no canonical-spelling entry.
#[test]
fn a_loaf_lays_out_the_identity_exports_its_install_record_carries() -> TestResult {
    use crate::typechecker::{TypeChecker, canonical_public_library_type_name};
    let root = tempfile::tempdir()?;
    let loaves = shapes(root.path())?;
    let manifest = Arc::clone(&loaves.entry("shapes").ok_or("missing shapes")?.artifact.manifest);
    let mut producer = TypeChecker::new();
    producer.set_loaves(Arc::clone(&loaves));
    let mut record = producer.loaf_install_record("shapes", &manifest);
    let before = record.identity_graph.exports.len();
    record
        .identity_graph
        .exports
        .retain(|export| export.public_name != "Unused");
    assert!(
        record.identity_graph.exports.len() < before,
        "the graph publishes Unused"
    );
    let outer = canonical_public_library_type_name("shapes", "Outer");
    let unused = canonical_public_library_type_name("shapes", "Unused");
    let cache = DependencyInterfacePreparationCache::default().with_install_record("shapes", record);
    let mut consumer = checker("consumer", &loaves, &cache);
    consumer.cache_transitive_pub_export_semantics("shapes");
    assert!(
        consumer.transitive_pub_types.contains_key(&outer),
        "a carried export is laid out"
    );
    assert!(
        !consumer.transitive_pub_types.contains_key(&unused),
        "an export the record's graph omits is laid out under {unused}"
    );
    Ok(())
}

/// A compiled Loaf's foreign preparation starts from the type origins its install record lists: given a record that
/// lists none, the Loaf's foreign remapping reaches no other Loaf's type.
#[test]
fn a_loafs_foreign_preparation_starts_from_the_origins_its_install_record_lists() -> TestResult {
    use crate::typechecker::TypeChecker;
    let root = tempfile::tempdir()?;
    let loaves = shapes(root.path())?;
    let manifest = Arc::clone(&loaves.entry("shapes").ok_or("missing shapes")?.artifact.manifest);
    let mut producer = TypeChecker::new();
    producer.set_loaves(Arc::clone(&loaves));
    let mut record = producer.loaf_install_record("shapes", &manifest);
    assert!(!record.type_origins.is_empty(), "shapes names a base type");
    let mut whole = checker("consumer", &loaves, &DependencyInterfacePreparationCache::default());
    let manifest_record = whole
        .prepared_install_record("shapes")
        .ok_or("missing shapes install record")?;
    assert!(
        !whole
            .public_library_foreign_type_remapping("shapes", &manifest_record)
            .is_empty(),
        "the whole record reaches base"
    );
    record.type_origins.clear();
    let cache = DependencyInterfacePreparationCache::default().with_install_record("shapes", record);
    let mut consumer = checker("consumer", &loaves, &cache);
    let record = consumer
        .prepared_install_record("shapes")
        .ok_or("missing injected shapes install record")?;
    let remapping = consumer.public_library_foreign_type_remapping("shapes", &record);
    assert!(remapping.is_empty(), "a record without origins reaches {remapping:?}");
    Ok(())
}

/// A compiled Loaf's foreign preparation lays out a reached Loaf's types from the identity graph of that Loaf's install
/// record: given a record of `base` whose graph omits `Other`, installing `shapes`, which reaches `base`, lays out
/// `Inner` under its qualified spelling and not `Other`.
#[test]
fn a_reached_loaf_is_laid_out_from_the_identity_graph_its_install_record_carries() -> TestResult {
    use crate::typechecker::{TypeChecker, canonical_public_library_type_name};
    let root = tempfile::tempdir()?;
    let loaves = shapes(root.path())?;
    let base = Arc::clone(&loaves.entry("base").ok_or("missing base")?.artifact.manifest);
    let mut producer = TypeChecker::new();
    producer.set_loaves(Arc::clone(&loaves));
    let mut record = producer.loaf_install_record("base", &base);
    record
        .identity_graph
        .exports
        .retain(|export| export.public_name != "Other");
    let inner = canonical_public_library_type_name("base", "Inner");
    let other = canonical_public_library_type_name("base", "Other");
    let mut whole = checker("consumer", &loaves, &DependencyInterfacePreparationCache::default());
    whole.cache_transitive_pub_export_semantics("shapes");
    assert!(
        whole.transitive_pub_types.contains_key(&other),
        "the whole record lays out {other}"
    );
    let cache = DependencyInterfacePreparationCache::default().with_install_record("base", record);
    let mut consumer = checker("consumer", &loaves, &cache);
    consumer.cache_transitive_pub_export_semantics("shapes");
    assert!(
        consumer.transitive_pub_types.contains_key(&inner),
        "a carried type is laid out"
    );
    assert!(
        !consumer.transitive_pub_types.contains_key(&other),
        "a type the reached Loaf's record omits is laid out under {other}"
    );
    Ok(())
}

/// Installing a sealed consumer interface from checked Loaf records preserves its dependency tables and refusals.
/// Its provider identities come from the admitted inventories without demanding the manifests before a layout read.
#[test]
fn a_loaf_install_record_replays_the_dependency_interface_without_opening_manifests() -> TestResult {
    use super::rust_reexport_chain_tests::published_module;
    use crate::library_manifest::install_record::LoafInstallRecord;
    use crate::typechecker::{TypeChecker, canonical_public_library_type_name};

    let root = tempfile::tempdir()?;
    let shapes_loaves = shapes(root.path())?;
    let base = shapes_loaves.entry("base").ok_or("missing base")?;
    let shapes = shapes_loaves.entry("shapes").ok_or("missing shapes")?;
    let specs = installed(
        root.path(),
        &published_module(
            "specs",
            "fields",
            "pub model Holder[T with Clone]:\n    pub item: T\n\npub type Saved = Holder[str]\n",
            LoafIndex::default(),
            Vec::new(),
        )?,
    )?;
    let loaves = Arc::new(LoafIndex::new([base.clone(), shapes.clone(), specs.clone()])?);
    let mut producer = checker("provider", &loaves, &DependencyInterfacePreparationCache::default());
    let provider = parse(PROVIDER)?;
    producer
        .check_program(&provider)
        .map_err(|errors| format!("{errors:?}"))?;
    producer.cache_transitive_pub_export_semantics("specs");
    let interface = producer.seal_current_module_dependency_interface(
        "provider",
        &provider,
        crate::module::exported_symbols(&provider).into(),
        true,
    );
    let cases = [Arc::clone(&loaves), Arc::new(LoafIndex::new([shapes.clone(), specs])?)];
    for (complete, legacy) in [true, false].into_iter().zip(cases) {
        let deferred = Arc::new(legacy.as_ref().clone().with_deferred_fixture_manifests()?);
        let mut expected = checker("consumer", &legacy, &DependencyInterfacePreparationCache::default());
        let mut replay = DependencyInterfacePreparationCache::default();
        for library in legacy.keys() {
            let manifest = legacy.manifest(library).ok_or("missing fixture manifest")?;
            let record = producer.loaf_install_record(library, manifest);
            let record = serde_json::from_slice::<LoafInstallRecord>(&serde_json::to_vec(&record)?)?;
            replay = replay.with_install_record(library, record);
        }
        let mut actual = checker("consumer", &deferred, &replay);
        expected.install_dependency_interfaces_directly(std::slice::from_ref(&interface));
        actual.install_dependency_interfaces_directly(std::slice::from_ref(&interface));

        let alias = TypeChecker::declaration_type_key("specs", &["fields".to_owned()], "Saved");
        assert!(expected.type_aliases.contains_key(&alias));
        assert_eq!(
            serde_json::to_vec(&actual.type_aliases.get(&alias))?,
            serde_json::to_vec(&expected.type_aliases.get(&alias))?
        );
        assert!(!expected.nominal_type_param_bounds.is_empty());
        assert_eq!(actual.nominal_type_param_bounds, expected.nominal_type_param_bounds);
        assert_eq!(actual.errors.is_empty(), complete);
        assert_eq!(format!("{:?}", actual.errors), format!("{:?}", expected.errors));
        if complete {
            let name = canonical_public_library_type_name("base", "Inner");
            let identity = actual
                .public_library_type_identities
                .get(&name)
                .ok_or("missing Inner identity")?;
            assert_eq!(identity.selected_provider.as_ref(), Some(&base.artifact.identity));
        }
        assert!(
            deferred.demanded_fixture_manifests().is_empty(),
            "install opened manifests before a layout read: {:?}",
            deferred.demanded_fixture_manifests()
        );
        assert_eq!(
            actual.dependency_interface_snapshot(),
            expected.dependency_interface_snapshot()
        );
    }
    Ok(())
}
