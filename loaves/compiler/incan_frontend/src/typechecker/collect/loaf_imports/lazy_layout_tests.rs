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
fn shapes(root: &Path) -> TestResult<Arc<LoafIndex>> {
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
