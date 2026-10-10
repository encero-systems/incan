//! Real Store publication/admission controls; fixture outputs are data and require no native compiler.

use super::{
    DOMAIN, EDGES_INPUT, NativeLoafClosure, NativeLoafDependency, NativeLoafError, NativeLoafGraph, NativeLoafOrigin,
    NativeLoafPhysicalBinding, NativeLoafPredicate, NativeLoafRecord, NativeLoafReference, NativeLoafRoot,
    NativeLoafSource, ORIGIN_INPUT, Result, SOURCE_INPUT, Store, physical_edges_input, prepare_resolved_native_loafs,
    record_receipt, retain_forward, select_owner, source_binding_input, verify_children, verify_native, verify_record,
};
use crate::plan::shared::OvenSharedNativeOwners;
use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreLimits,
    PublishedOvenStore,
};
use oven_store::{
    OvenGeneratedProjectRequest, OvenReceipt, digest_bytes, receipt_generated_project, receipt_with_build_unit_input,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Publish a real native Engine recipe and its ordinary record, retaining original leases.
fn publish(
    root: &Path,
    store: &OvenStore,
    graph: &mut NativeLoafGraph,
    name: &str,
    features: &[&str],
    dependencies: &[(&str, &str)],
) -> TestResult<String> {
    publish_with_domain(
        root,
        store,
        graph,
        name,
        features,
        dependencies,
        "fixture-native-target",
    )
}

/// Select a native publication domain explicitly to cover both ordinary and migration proof policies.
fn publish_with_domain(
    root: &Path,
    store: &OvenStore,
    graph: &mut NativeLoafGraph,
    name: &str,
    features: &[&str],
    dependencies: &[(&str, &str)],
    domain: &str,
) -> TestResult<String> {
    let project = root.join(name);
    std::fs::create_dir_all(&project)?;
    let source_file = project.join("lib.rs");
    std::fs::write(&source_file, format!("pub const SOURCE: &str = {name:?};\n"))?;
    let source = NativeLoafSource {
        loaf: format!("fixture/{name}"),
        version: "1.0.0".to_string(),
        archive_digest: digest_bytes(&std::fs::read(&source_file)?),
        domain: "target".to_string(),
        features: features.iter().map(|feature| feature.to_string()).collect(),
        target_predicates: vec![NativeLoafPredicate {
            declaration: 2,
            target: "cfg(unix)".to_string(),
            matches: true,
        }],
    };
    let mut request = OvenGeneratedProjectRequest::new(
        &project,
        &source.loaf,
        &source.version,
        "fixture-target",
        "fixture-toolchain",
        "debug",
        source.features.clone(),
    )
    .with_generated_source("native-root", &source_file)
    .with_build_unit_input("source-generation", &source.archive_digest)
    .with_build_unit_input("domain", &source.domain)
    .with_build_unit_input(ORIGIN_INPUT, NativeLoafOrigin::Registry.as_str())
    .with_build_unit_input(SOURCE_INPUT, source_binding_input(&source)?);
    let mut edges = Vec::new();
    for (alias, identity) in dependencies {
        let child = graph.units.get(*identity).ok_or("fixture child record missing")?;
        request = request.with_build_unit_input(format!("extern:{alias}"), &child.record.native.digest);
        edges.push(NativeLoafDependency {
            alias: alias.to_string(),
            record_identity: identity.to_string(),
            native: child.record.native.clone(),
            source: child.record.source.clone(),
        });
    }
    let bindings = edges
        .iter()
        .map(|edge| NativeLoafPhysicalBinding {
            alias: edge.alias.clone(),
            source: edge.source.clone(),
            native: edge.native.clone(),
        })
        .collect::<Vec<_>>();
    request = request.with_build_unit_input(EDGES_INPUT, physical_edges_input(&bindings)?);
    let recipe = receipt_generated_project(&request)?;
    let output = project.join("libfixture.rlib");
    std::fs::write(&output, b"identical native bytes across different source owners")?;
    let manifest = store.publish(&OvenArtifactPublishRequest {
        receipt: recipe.clone(),
        domain: domain.to_string(),
        kind: OvenArtifactKind::Engine,
        payload: serde_json::to_vec(&source)?,
        materialized_files: vec![OvenArtifactMaterializedFile {
            source_path: output,
            relative_path: "libfixture.rlib".to_string(),
        }],
        materialized_directories: Vec::new(),
    })?;
    let owner = select_owner(Store::Writable(store), &manifest.identity)?;
    let native = NativeLoafReference {
        identity: manifest.identity,
        receipt_identity: recipe.identity.clone(),
        domain: manifest.domain,
        relative_path: "libfixture.rlib".to_string(),
        digest: manifest
            .materialized_files
            .first()
            .ok_or("fixture output missing")?
            .digest
            .clone(),
    };
    Ok(graph.publish(store, source, native, recipe, owner, edges)?)
}

/// Open a bounded Store that can retain every small fixture record and native member.
fn store(root: &Path) -> OvenStore {
    OvenStore::new(
        root,
        OvenStoreLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 16 * 1024 * 1024),
    )
}

/// Publish inventoried source declarations with explicit producer provenance for root-selection controls.
fn publish_declaration(
    root: &Path,
    store: &OvenStore,
    graph: &mut NativeLoafGraph,
    manifest: &toml::Value,
    archive_digest: &str,
    features: &[&str],
    origin: NativeLoafOrigin,
    domain: &str,
) -> TestResult<String> {
    let project = root.join(format!("publisher-{}", graph.units.len()));
    std::fs::create_dir_all(&project)?;
    let declaration = project.join("loaf.toml");
    std::fs::write(&declaration, toml::to_string(manifest)?)?;
    let source = NativeLoafSource {
        loaf: manifest["project"]["name"]
            .as_str()
            .ok_or("fixture name missing")?
            .to_string(),
        version: manifest["project"]["version"]
            .as_str()
            .ok_or("fixture version missing")?
            .to_string(),
        archive_digest: archive_digest.to_string(),
        domain: domain.to_string(),
        features: features.iter().map(|feature| feature.to_string()).collect(),
        target_predicates: Vec::new(),
    };
    let recipe = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            &project,
            &source.loaf,
            &source.version,
            "fixture-target",
            "fixture-toolchain",
            "debug",
            source.features.clone(),
        )
        .with_generated_source("source-declaration", &declaration)
        .with_build_unit_input("domain", domain)
        .with_build_unit_input(ORIGIN_INPUT, origin.as_str())
        .with_build_unit_input(SOURCE_INPUT, source_binding_input(&source)?)
        .with_build_unit_input(EDGES_INPUT, physical_edges_input(&[])?),
    )?;
    let output = project.join("libfixture.rlib");
    std::fs::write(&output, b"identical native output, independently sealed declaration")?;
    let published = store.publish(&OvenArtifactPublishRequest {
        receipt: recipe.clone(),
        domain: "ordinary-fixture-native".to_string(),
        kind: OvenArtifactKind::Engine,
        payload: serde_json::to_vec(&source)?,
        materialized_files: vec![
            OvenArtifactMaterializedFile {
                source_path: output,
                relative_path: "libfixture.rlib".to_string(),
            },
            OvenArtifactMaterializedFile {
                source_path: declaration,
                relative_path: "source/loaf.toml".to_string(),
            },
        ],
        materialized_directories: Vec::new(),
    })?;
    let native = NativeLoafReference {
        identity: published.identity.clone(),
        receipt_identity: recipe.identity.clone(),
        domain: published.domain,
        relative_path: "libfixture.rlib".to_string(),
        digest: published
            .materialized_files
            .iter()
            .find(|member| member.relative_path == "libfixture.rlib")
            .ok_or("fixture output missing")?
            .digest
            .clone(),
    };
    let owner = select_owner(Store::Writable(store), &published.identity)?;
    Ok(graph.publish(store, source, native, recipe, owner, Vec::new())?)
}

/// Construct a genuine local mapped source tree, including authored defaults and feature aliases.
fn local_declaration(root: &Path) -> TestResult<PathBuf> {
    let project = root.join("current");
    std::fs::create_dir_all(project.join("src"))?;
    std::fs::write(project.join("src/lib.rs"), "pub const CURRENT: u8 = 1;\n")?;
    std::fs::write(
        project.join("loaf.toml"),
        "[project]\nname='local_fixture'\nversion='1.0.0'\n[project.features]\ndefault=['enabled']\nenabled=[]\nextra=[]\n[rust]\nname='local_fixture'\n",
    )?;
    Ok(project)
}

/// Build an active declaration without invoking a resolver or changing its optional activation semantics.
fn dependency(alias: &str, source: oven_model::manifest::DependencySource) -> oven_model::manifest::DependencySpec {
    oven_model::manifest::DependencySpec {
        crate_name: alias.to_string(),
        version: Some("^1.0".to_string()),
        features: Vec::new(),
        default_features: true,
        source,
        optional: false,
        package: None,
    }
}

/// Current local source/default closure is mandatory, and two declared aliases retain the same original owner.
#[test]
fn dev7_native_loaf_declaration_local_aliases_features_and_source_restoration() -> TestResult {
    let temp = tempfile::tempdir()?;
    let native = store(&temp.path().join("store"));
    let current = local_declaration(temp.path())?;
    let (manifest, digest) = crate::sdk_closure::local_native_source_selection(&current)?;
    let mut graph = NativeLoafGraph::default();
    let identity = publish_declaration(
        temp.path(),
        &native,
        &mut graph,
        &manifest,
        &digest,
        &["default", "enabled"],
        NativeLoafOrigin::Local,
        "target",
    )?;
    let declaration = dependency(
        "renamed-local",
        oven_model::manifest::DependencySource::Path {
            path: PathBuf::from("current"),
        },
    );
    let mut second = declaration.clone();
    second.crate_name = "another_alias".to_string();
    let roots = graph.select_dependency_roots(&[declaration.clone(), second], temp.path(), "target")?;
    assert_eq!(
        roots.iter().map(|root| root.alias.as_str()).collect::<Vec<_>>(),
        ["another_alias", "renamed_local"]
    );
    assert!(
        roots
            .iter()
            .all(|root| root.record_identity == identity && root.source.features == ["default", "enabled"])
    );
    let closure = graph.select(&roots)?;
    assert_eq!(closure.graph.units.len(), 1);
    assert!(Arc::ptr_eq(
        graph.units.get(&identity).ok_or("original missing")?,
        closure.graph.units.get(&identity).ok_or("selected missing")?
    ));
    let source = current.join("src/lib.rs");
    let original = std::fs::read(&source)?;
    std::fs::write(&source, "pub const CURRENT: u8 = 2;\n")?;
    refuses(
        graph.select_dependency_roots(std::slice::from_ref(&declaration), temp.path(), "target"),
        "no current",
    )?;
    std::fs::write(&source, original)?;
    assert_eq!(
        graph
            .select_dependency_roots(std::slice::from_ref(&declaration), temp.path(), "target")?
            .len(),
        1
    );
    let mut missing_feature = declaration.clone();
    missing_feature.features.push("extra".to_string());
    refuses(
        graph.select_dependency_roots(&[missing_feature], temp.path(), "target"),
        "no current",
    )?;
    let mut wrong_package = declaration.clone();
    wrong_package.package = Some("some_other_source".to_string());
    refuses(
        graph.select_dependency_roots(&[wrong_package], temp.path(), "target"),
        "contradicts",
    )?;
    refuses(
        graph.select_dependency_roots(&[declaration], temp.path(), "host"),
        "no current",
    )?;
    Ok(())
}

/// Registry rename/version matching and macro domains use admitted declarations; incompatible origins never match.
#[test]
fn dev7_native_loaf_declaration_registry_macro_origin_and_ambiguity() -> TestResult {
    let temp = tempfile::tempdir()?;
    let native = store(&temp.path().join("store"));
    let manifest: toml::Value = toml::from_str(
        "[project]\nname='crates-io/macros'\nversion='1.0.0'\n[project.features]\nextra=[]\n[rust]\ntype='proc-macro'\n",
    )?;
    let mut graph = NativeLoafGraph::default();
    let identity = publish_declaration(
        temp.path(),
        &native,
        &mut graph,
        &manifest,
        "sha256:registry-fixture",
        &[],
        NativeLoafOrigin::Registry,
        "host",
    )?;
    let mut renamed = dependency("renamed_macro", oven_model::manifest::DependencySource::Registry);
    renamed.package = Some("macros".to_string());
    let roots = graph.select_dependency_roots(std::slice::from_ref(&renamed), temp.path(), "target")?;
    assert_eq!(roots[0].source.domain, "host");
    assert_eq!(roots[0].record_identity, identity);
    let mut wrong_version = renamed.clone();
    wrong_version.version = Some("^2.0".to_string());
    refuses(
        graph.select_dependency_roots(&[wrong_version], temp.path(), "target"),
        "no current",
    )?;
    let mut missing = renamed.clone();
    missing.package = Some("absent".to_string());
    missing.optional = true;
    refuses(
        graph.select_dependency_roots(&[missing], temp.path(), "target"),
        "no current",
    )?;
    publish_declaration(
        temp.path(),
        &native,
        &mut graph,
        &manifest,
        "sha256:different-registry-fixture",
        &["extra"],
        NativeLoafOrigin::Registry,
        "host",
    )?;
    refuses(
        graph.select_dependency_roots(&[renamed], temp.path(), "target"),
        "ambiguous",
    )?;

    let current = local_declaration(temp.path())?;
    let (local_manifest, digest) = crate::sdk_closure::local_native_source_selection(&current)?;
    let mut wrong_origin = NativeLoafGraph::default();
    publish_declaration(
        &temp.path().join("wrong-origin"),
        &native,
        &mut wrong_origin,
        &local_manifest,
        &digest,
        &["default", "enabled"],
        NativeLoafOrigin::Registry,
        "target",
    )?;
    let local = dependency("local", oven_model::manifest::DependencySource::Path { path: current });
    refuses(
        wrong_origin.select_dependency_roots(&[local], temp.path(), "target"),
        "no current",
    )?;
    assert!(graph.select_dependency_roots(&[], temp.path(), "target")?.is_empty());
    Ok(())
}

/// A prepared facet lacking the authored default closure cannot satisfy a default-enabled declaration.
#[test]
fn dev7_native_loaf_declaration_requires_defaults_and_admitted_manifest() -> TestResult {
    let temp = tempfile::tempdir()?;
    let native = store(&temp.path().join("store"));
    let current = local_declaration(temp.path())?;
    let (manifest, digest) = crate::sdk_closure::local_native_source_selection(&current)?;
    let mut graph = NativeLoafGraph::default();
    publish_declaration(
        temp.path(),
        &native,
        &mut graph,
        &manifest,
        &digest,
        &[],
        NativeLoafOrigin::Local,
        "target",
    )?;
    let mut declaration = dependency("local", oven_model::manifest::DependencySource::Path { path: current });
    refuses(
        graph.select_dependency_roots(std::slice::from_ref(&declaration), temp.path(), "target"),
        "no current",
    )?;
    declaration.default_features = false;
    assert_eq!(
        graph
            .select_dependency_roots(std::slice::from_ref(&declaration), temp.path(), "target")?
            .len(),
        1
    );
    let unit = graph.units.values().next().ok_or("fixture missing")?;
    replace_owned_fixture(
        &unit.native_owner.artifact_root.join("source/loaf.toml"),
        b"[project]\nname='substituted'\n",
    )?;
    assert!(
        graph
            .select_dependency_roots(&[declaration], temp.path(), "target")
            .is_err()
    );
    Ok(())
}

/// A malformed explicit graph refuses before compiler acquisition, output creation or ambient discovery.
#[test]
fn dev7_native_loaf_explicit_graph_refuses_unknown_authority_before_preparation() -> TestResult {
    let temp = tempfile::tempdir()?;
    let graph = temp.path().join("native-graph.json");
    std::fs::write(
        &graph,
        r#"{"index_commit":"fixture","registry_lock":"lock.json","facets":[],"sdk_inventory":"ambient"}"#,
    )?;
    let absent = temp.path().join("absent");
    let output = temp.path().join("output");
    let error = prepare_resolved_native_loafs(&graph, &absent, &absent, &output, &absent, "fixture-target", "debug")
        .err()
        .ok_or("unknown graph authority accepted")?;
    assert!(
        error.to_string().contains("unknown field"),
        "unexpected graph refusal: {error}"
    );
    assert!(!output.exists());
    Ok(())
}

/// Project an exact declared root from already checked fixture authority.
fn root(graph: &NativeLoafGraph, identity: &str, alias: &str) -> TestResult<NativeLoafRoot> {
    let unit = graph.units.get(identity).ok_or("fixture root missing")?;
    Ok(NativeLoafRoot {
        alias: alias.to_string(),
        record_identity: identity.to_string(),
        source: unit.record.source.clone(),
        intent: unit.record.recipe.intent.clone(),
    })
}

/// Check a definitive refusal rather than accepting an unrelated I/O failure as semantic coverage.
fn refuses<T>(result: Result<T>, family: &str) -> TestResult {
    let error = result.err().ok_or("invalid native admission succeeded")?;
    assert!(
        matches!(&error, NativeLoafError::Refused(message) if message.contains(family)),
        "expected refusal {family:?}, got {error}"
    );
    Ok(())
}

/// Mutate only a test-owned immutable member, retaining its permissions and modified time for integrity controls.
pub(super) fn replace_owned_fixture(path: &Path, bytes: &[u8]) -> TestResult {
    let metadata = std::fs::metadata(path)?;
    let original = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(original.mode() | 0o200))?;
    }
    #[cfg(not(unix))]
    {
        let mut writable = original.clone();
        writable.set_readonly(false);
        std::fs::set_permissions(path, writable)?;
    }
    let result = (|| -> std::io::Result<()> {
        std::fs::write(path, bytes)?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)?
            .set_times(std::fs::FileTimes::new().set_modified(metadata.modified()?))
    })();
    std::fs::set_permissions(path, original)?;
    result?;
    Ok(())
}

/// Copy a published Store without rewriting identities, proving durable records are portable across installation.
fn copy_tree(source: &Path, destination: &Path) -> TestResult {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Exact aliases and real forward edges select a subset; reverse invalidation never expands linking roots.
#[test]
fn dev7_native_loaf_declared_closure_retains_aliases_and_original_leases() -> TestResult {
    let temp = tempfile::tempdir()?;
    let native = store(&temp.path().join("store"));
    let mut graph = NativeLoafGraph::default();
    let leaf = publish(temp.path(), &native, &mut graph, "leaf", &["enabled"], &[])?;
    let parent = publish(
        temp.path(),
        &native,
        &mut graph,
        "parent",
        &[],
        &[("renamed_leaf", &leaf)],
    )?;
    let unrelated = publish(temp.path(), &native, &mut graph, "unrelated", &[], &[])?;
    let roots = [
        root(&graph, &parent, "first_alias")?,
        root(&graph, &parent, "second_alias")?,
    ];
    let selected = graph.select(&roots)?;
    assert_eq!(selected.roots.len(), 2);
    assert_eq!(selected.graph.units.len(), 2);
    assert!(!selected.graph.units.contains_key(&unrelated));
    let original = graph.units.get(&leaf).ok_or("leaf missing")?;
    let retained = selected.graph.units.get(&leaf).ok_or("retained leaf missing")?;
    assert!(Arc::ptr_eq(original, retained));
    assert!(Arc::ptr_eq(&original.native_owner, &retained.native_owner));
    assert_eq!(
        selected
            .graph
            .units
            .get(&parent)
            .ok_or("parent missing")?
            .record
            .dependencies[0]
            .alias,
        "renamed_leaf"
    );
    assert_eq!(
        graph.affected_dependents(&BTreeSet::from([leaf.clone()]))?,
        BTreeSet::from([leaf.clone(), parent.clone()])
    );
    assert_eq!(graph.select(&[root(&graph, &leaf, "leaf_only")?])?.graph.units.len(), 1);
    let reference = selected.shared_root(&leaf, "native/leaf")?;
    assert_eq!(reference.identity, original.record.native.identity);
    let held = selected.shared_owners()?;
    drop(graph);
    let bounded = OvenStore::new(native.root(), OvenStoreLimits::new(1, 1, 1));
    let report = bounded.prune()?;
    assert!(!report.removed_entries.contains(&leaf));
    assert!(!report.removed_entries.contains(&parent));
    assert!(retained.output()?.is_file());
    let output = retained.output()?;
    drop(held);
    drop(selected);
    bounded.prune()?;
    assert!(!output.exists());
    Ok(())
}

/// Installed consumers use durable per-unit records without any SDK inventory or command-live producer object.
#[test]
fn dev7_native_loaf_published_store_admits_portable_records() -> TestResult {
    let temp = tempfile::tempdir()?;
    let native = store(&temp.path().join("source-store"));
    let mut graph = NativeLoafGraph::default();
    let child = publish(temp.path(), &native, &mut graph, "child", &["test_support"], &[])?;
    let parent = publish(temp.path(), &native, &mut graph, "parent", &[], &[("renamed", &child)])?;
    let roots = [root(&graph, &parent, "consumer")?];
    let installed = temp.path().join("installed-store");
    copy_tree(native.root(), &installed)?;
    drop(graph);
    let admitted = NativeLoafClosure::admit_published(&PublishedOvenStore::new(&installed), &roots)?;
    assert_eq!(admitted.graph.units.len(), 2);
    for unit in admitted.graph.units.values() {
        assert!(unit.output()?.starts_with(std::fs::canonicalize(&installed)?));
        assert!(unit.record_owner.original_native_receipt().is_none());
        assert!(unit.native_owner.original_native_receipt().is_none());
    }
    let ordinary = NativeLoafClosure::admit(&store(&installed), &roots)?;
    assert_eq!(ordinary.roots, admitted.roots);
    assert_eq!(
        ordinary.graph.units.keys().collect::<Vec<_>>(),
        admitted.graph.units.keys().collect::<Vec<_>>()
    );
    Ok(())
}

/// Source/feature/domain/predicate and full target/toolchain/profile identity remain exact root requirements.
#[test]
fn dev7_native_loaf_refuses_changed_declared_generation_and_intent() -> TestResult {
    let temp = tempfile::tempdir()?;
    let native = store(&temp.path().join("store"));
    let mut graph = NativeLoafGraph::default();
    let identity = publish(temp.path(), &native, &mut graph, "fixture", &["test_support"], &[])?;
    let expected = root(&graph, &identity, "fixture")?;
    let mut changes = Vec::new();
    let mut changed = expected.clone();
    changed.source.archive_digest = digest_bytes(b"changed source");
    changes.push(changed);
    let mut changed = expected.clone();
    changed.source.domain = "host".to_string();
    changes.push(changed);
    let mut changed = expected.clone();
    changed.source.features.clear();
    changes.push(changed);
    let mut changed = expected.clone();
    changed.source.target_predicates[0].matches = false;
    changes.push(changed);
    let mut changed = expected.clone();
    changed.intent.target = "other-target".to_string();
    changes.push(changed);
    let mut changed = expected.clone();
    changed.intent.toolchain = "other-toolchain".to_string();
    changes.push(changed);
    let mut changed = expected.clone();
    changed.intent.profile = "release".to_string();
    changes.push(changed);
    for changed in changes {
        refuses(graph.select(&[changed]), "source or intent")?;
    }
    refuses(graph.select(&[expected.clone(), expected]), "alias")?;
    Ok(())
}

/// Equal native bytes cannot substitute another immutable child owner; missing and injected edges refuse.
#[test]
fn dev7_native_loaf_refuses_substituted_and_injected_physical_edges() -> TestResult {
    let temp = tempfile::tempdir()?;
    let native = store(&temp.path().join("store"));
    let mut graph = NativeLoafGraph::default();
    let child = publish(temp.path(), &native, &mut graph, "child", &[], &[])?;
    let other = publish(temp.path(), &native, &mut graph, "other", &["different"], &[])?;
    let parent = publish(temp.path(), &native, &mut graph, "parent", &[], &[("renamed", &child)])?;
    let unit = graph.units.get(&parent).ok_or("parent missing")?;
    let mut substituted = unit.record.clone();
    let other = graph.units.get(&other).ok_or("other missing")?;
    assert_eq!(substituted.dependencies[0].native.digest, other.record.native.digest);
    assert_ne!(
        substituted.dependencies[0].native.identity,
        other.record.native.identity
    );
    substituted.dependencies[0].native = other.record.native.clone();
    refuses(verify_children(&substituted, &graph.units), "owner was substituted")?;
    let mut injected = unit.record.clone();
    injected.dependencies[0].alias = "injected".to_string();
    refuses(verify_native(&injected, &unit.native_owner, false), "extern recipe")?;
    let mut missing = unit.record.clone();
    missing.dependencies.clear();
    refuses(verify_native(&missing, &unit.native_owner, false), "extern recipe")?;
    let mut absent = BTreeMap::new();
    absent.insert(parent.clone(), Arc::clone(unit));
    refuses(
        retain_forward(&parent, &absent, &mut BTreeMap::new(), &mut BTreeSet::new()),
        "dependency record is missing",
    )?;
    let mut recipe = unit.record.clone();
    recipe.recipe.intent.target = "substituted-target".to_string();
    assert!(verify_record(&recipe, &unit.record_owner, &unit.native_owner, false).is_err());
    Ok(())
}

/// Missing durable authority is explicit, and held records/native bytes are rechecked at physical handoff.
#[test]
fn dev7_native_loaf_missing_and_corrupt_owners_refuse_without_fallback() -> TestResult {
    let temp = tempfile::tempdir()?;
    let native = store(&temp.path().join("store"));
    let mut graph = NativeLoafGraph::default();
    let identity = publish(temp.path(), &native, &mut graph, "fixture", &[], &[])?;
    let expected = root(&graph, &identity, "fixture")?;
    let mut missing = expected.clone();
    missing.record_identity = digest_bytes(b"absent record");
    assert!(matches!(
        NativeLoafClosure::admit(&native, &[missing]),
        Err(NativeLoafError::Unavailable { .. })
    ));
    let selected = graph.select(&[expected])?;
    let unit = selected.graph.units.get(&identity).ok_or("unit missing")?;
    let installed = temp.path().join("missing-native-store");
    copy_tree(native.root(), &installed)?;
    let source_store = std::fs::canonicalize(native.root())?;
    let entry = unit.native_owner.artifact_root.parent().ok_or("native entry missing")?;
    std::fs::remove_dir_all(installed.join(entry.strip_prefix(&source_store)?))?;
    assert!(matches!(
        NativeLoafClosure::admit_published(
            &PublishedOvenStore::new(&installed),
            &[root(&graph, &identity, "fixture")?]
        ),
        Err(NativeLoafError::Unavailable { .. })
    ));
    let record_payload = unit
        .record_owner
        .artifact_root
        .parent()
        .ok_or("record entry missing")?
        .join("payload");
    let published = NativeLoafClosure::admit_published(
        &PublishedOvenStore::new(native.root()),
        &[root(&graph, &identity, "fixture")?],
    )?;
    let published_unit = published.graph.units.get(&identity).ok_or("published unit missing")?;
    let native_payload = unit
        .native_owner
        .artifact_root
        .parent()
        .ok_or("native entry missing")?
        .join("payload");
    for path in [&record_payload, &native_payload, &unit.output()?] {
        let original = std::fs::read(path)?;
        let mut changed = original.clone();
        *changed.first_mut().ok_or("fixture bytes missing")? ^= 1;
        replace_owned_fixture(path, &changed)?;
        for held in [unit, published_unit] {
            assert!(
                held.output().is_err(),
                "changed held bytes accepted: {}",
                path.display()
            );
        }
        assert!(selected.shared_owners().is_err());
        assert!(published.shared_owners().is_err());
        replace_owned_fixture(path, &original)?;
        assert!(unit.output()?.is_file());
        assert!(published_unit.output()?.is_file());
    }
    for path in [&record_payload, &native_payload] {
        let moved = temp.path().join("removed-payload");
        std::fs::rename(path, &moved)?;
        assert!(unit.output().is_err());
        assert!(published_unit.output().is_err());
        std::fs::rename(&moved, path)?;
        assert!(unit.output()?.is_file());
        assert!(published_unit.output()?.is_file());
        #[cfg(unix)]
        {
            std::fs::rename(path, &moved)?;
            std::os::unix::fs::symlink(&moved, path)?;
            assert!(unit.output().is_err(), "matching foreign payload symlink accepted");
            assert!(published_unit.output().is_err());
            std::fs::remove_file(path)?;
            std::fs::rename(&moved, path)?;
            assert!(unit.output()?.is_file());
            assert!(published_unit.output()?.is_file());
        }
    }
    Ok(())
}

/// Record and native owner Stores cannot be inferred or exchanged across independently published locations.
#[test]
fn dev7_native_loaf_cross_store_distribution_is_explicitly_unavailable() -> TestResult {
    let temp = tempfile::tempdir()?;
    let native = store(&temp.path().join("store"));
    let mut graph = NativeLoafGraph::default();
    let identity = publish(temp.path(), &native, &mut graph, "fixture", &[], &[])?;
    let unit = graph.units.get(&identity).ok_or("unit missing")?;
    let different = store(&temp.path().join("different-store"));
    std::fs::create_dir_all(different.root())?;
    refuses(
        NativeLoafGraph::default().publish(
            &different,
            unit.record.source.clone(),
            unit.record.native.clone(),
            unit.record.recipe.clone(),
            Arc::clone(&unit.native_owner),
            Vec::new(),
        ),
        "same Store",
    )?;
    Ok(())
}

/// Publish a normal shared direct plan for one ordinary owner through existing public planner contracts.
fn consumer_plan(store: &OvenStore, selected: &NativeLoafClosure, identity: &str) -> TestResult<OvenReceipt> {
    use crate::plan::shared::OvenSharedNativePlan;
    use crate::rustc::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactExtern, OvenRustcArtifactManifest,
        OvenRustcSourceSearchClosure,
    };
    let unit = selected.graph.units.get(identity).ok_or("consumer unit missing")?;
    let receipt = unit.record.recipe.clone();
    let prefix = "units/fixture";
    let relative = format!("{prefix}/{}", unit.record.native.relative_path);
    let paths = vec![prefix.to_string()];
    let declared = BTreeMap::from([(relative.clone(), unit.record.native.digest.clone())]);
    let artifacts = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: paths.clone(),
        native_search_paths: Vec::new(),
        externs: vec![OvenRustcArtifactExtern {
            crate_name: "fixture".to_string(),
            relative_path: relative,
            digest: unit.record.native.digest.clone(),
        }],
        entrypoint_externs: BTreeMap::from([("generated-root".to_string(), vec!["fixture".to_string()])]),
        entrypoint_dependency_search_paths: BTreeMap::from([(
            "generated-root".to_string(),
            OvenRustcSourceSearchClosure::publisher_selected(paths, &declared),
        )]),
        registry_leaves: Vec::new(),
        registry_sources: Vec::new(),
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: Vec::new(),
    };
    store.publish(&OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: "ordinary-native-consumer-plan".to_string(),
        kind: OvenArtifactKind::DirectRustcPlan,
        payload: serde_json::to_vec(&OvenSharedNativePlan {
            artifacts,
            shared_native_roots: vec![selected.shared_root(identity, prefix)?],
        })?,
        materialized_files: Vec::new(),
        materialized_directories: Vec::new(),
    })?;
    Ok(receipt)
}

/// Only record-bound native domains cross ordinary handoff; legacy owner indexes remain conservative.
#[test]
fn dev7_native_loaf_record_bound_domain_uses_existing_shared_handoff() -> TestResult {
    use crate::plan::selection::select_receipt_direct_rustc_execution_plan_with_native_owners;
    let temp = tempfile::tempdir()?;
    let native = store(&temp.path().join("native-store"));
    let consumer = store(&temp.path().join("consumer-store"));
    let mut graph = NativeLoafGraph::default();
    let identity = publish(temp.path(), &native, &mut graph, "fixture", &[], &[])?;
    let selected = graph.select(&[root(&graph, &identity, "fixture")?])?;
    let receipt = consumer_plan(&consumer, &selected, &identity)?;
    let unit = selected.graph.units.get(&identity).ok_or("selected unit missing")?;
    let legacy = OvenSharedNativeOwners::from_selected(&[Arc::clone(&unit.native_owner)])?;
    assert!(select_receipt_direct_rustc_execution_plan_with_native_owners(&consumer, &receipt, Some(&legacy)).is_err());
    let admitted = selected.shared_owners()?;
    let plan = select_receipt_direct_rustc_execution_plan_with_native_owners(&consumer, &receipt, Some(&admitted))?
        .ok_or("ordinary shared plan missing")?;
    assert_eq!(plan.physical_path("units/fixture/libfixture.rlib"), unit.output()?);
    Ok(())
}

/// Read-only installed handoff works with absent closure proofs and never creates them in the installed Store.
#[test]
fn dev7_native_loaf_installed_handoff_does_not_write_proofs() -> TestResult {
    use crate::plan::selection::select_receipt_direct_rustc_execution_plan_with_native_owners;
    let temp = tempfile::tempdir()?;
    let native = store(&temp.path().join("native-store"));
    let consumer = store(&temp.path().join("consumer-store"));
    let mut graph = NativeLoafGraph::default();
    let identity = publish_with_domain(
        temp.path(),
        &native,
        &mut graph,
        "fixture",
        &[],
        &[],
        "sdk-source-unit-target",
    )?;
    let expected = root(&graph, &identity, "fixture")?;
    let installed = temp.path().join("installed-store");
    copy_tree(native.root(), &installed)?;
    // Removing the writable publisher's proof makes any attempted installed proof write observable.
    let proof_root = installed.join(oven_store::closure_proof::CLOSURE_PROOF_DIRECTORY);
    if proof_root.exists() {
        std::fs::remove_dir_all(&proof_root)?;
    }
    let selected = NativeLoafClosure::admit_published(&PublishedOvenStore::new(&installed), &[expected])?;
    let receipt = consumer_plan(&consumer, &selected, &identity)?;
    let admitted = selected.shared_owners()?;
    let _plan = select_receipt_direct_rustc_execution_plan_with_native_owners(&consumer, &receipt, Some(&admitted))?
        .ok_or("installed ordinary plan missing")?;
    assert!(!proof_root.exists());
    Ok(())
}

/// Simulate an independently re-sealed graph record while preserving the genuine unchanged native owner.
fn reseal(store: &OvenStore, record: &NativeLoafRecord) -> TestResult<NativeLoafRoot> {
    let payload = serde_json::to_vec(record)?;
    let receipt = record_receipt(record, &payload)?;
    let manifest = store.publish(&OvenArtifactPublishRequest {
        receipt,
        domain: DOMAIN.to_string(),
        kind: OvenArtifactKind::Engine,
        payload,
        materialized_files: Vec::new(),
        materialized_directories: Vec::new(),
    })?;
    Ok(NativeLoafRoot {
        alias: "forged".to_string(),
        record_identity: manifest.identity,
        source: record.source.clone(),
        intent: record.recipe.intent.clone(),
    })
}

/// An outer graph seal cannot authenticate forged source facts or same-byte substituted child owners.
#[test]
fn dev7_native_loaf_resealed_source_and_child_substitution_refuse() -> TestResult {
    let temp = tempfile::tempdir()?;
    let native = store(&temp.path().join("store"));
    let mut graph = NativeLoafGraph::default();
    let child = publish(temp.path(), &native, &mut graph, "child", &[], &[])?;
    let other = publish(temp.path(), &native, &mut graph, "other", &["different"], &[])?;
    let parent = publish(temp.path(), &native, &mut graph, "parent", &[], &[("renamed", &child)])?;
    let original = &graph.units.get(&parent).ok_or("parent missing")?.record;
    let mut source_forgery = original.clone();
    source_forgery.source.archive_digest = digest_bytes(b"forged source");
    let mut domain_forgery = original.clone();
    domain_forgery.source.domain = "host".to_string();
    let mut predicate_forgery = original.clone();
    predicate_forgery.source.target_predicates[0].matches = false;
    for forged in [source_forgery, domain_forgery, predicate_forgery] {
        let root = reseal(&native, &forged)?;
        refuses(NativeLoafClosure::admit(&native, &[root]), "sealed source")?;
    }
    let other_unit = graph.units.get(&other).ok_or("other missing")?;
    assert_eq!(original.dependencies[0].native.digest, other_unit.record.native.digest);
    let mut forged = original.clone();
    forged.dependencies[0].record_identity = other;
    forged.dependencies[0].native = other_unit.record.native.clone();
    forged.dependencies[0].source = other_unit.record.source.clone();
    let root = reseal(&native, &forged)?;
    refuses(NativeLoafClosure::admit(&native, &[root]), "owner coordinates")?;
    Ok(())
}

/// Recompute a genuinely canonical forged receipt so refusal does not rely on a trivially invalid hash.
fn canonicalize_receipt(receipt: &mut OvenReceipt) -> TestResult {
    receipt.identity = oven_store::receipt_identity(
        &receipt.project,
        &receipt.sources,
        &receipt.intent,
        &receipt.compatibility,
    )?;
    receipt.build_unit_identity = oven_store::build_unit_identity(
        &receipt.intent,
        &receipt.compatibility,
        &receipt.sources.build_unit_inputs,
    )?;
    receipt.verify_identity()?;
    Ok(())
}

/// Canonical graph receipts and native recipes with exchanged intents cannot bless the unchanged native owner.
#[test]
fn dev7_native_loaf_resealed_recipe_and_graph_intents_refuse() -> TestResult {
    let temp = tempfile::tempdir()?;
    let native = store(&temp.path().join("store"));
    let mut graph = NativeLoafGraph::default();
    let identity = publish(temp.path(), &native, &mut graph, "fixture", &[], &[])?;
    let record = &graph.units.get(&identity).ok_or("fixture missing")?.record;
    let mut forged = record.clone();
    forged.recipe.intent.target = "forged-target".to_string();
    canonicalize_receipt(&mut forged.recipe)?;
    let root = reseal(&native, &forged)?;
    refuses(
        NativeLoafClosure::admit(&native, &[root]),
        "sealed source, receipt or intent",
    )?;
    let mut forged_origin = record.clone();
    forged_origin.recipe = receipt_with_build_unit_input(&forged_origin.recipe, ORIGIN_INPUT, "local")?;
    let root = reseal(&native, &forged_origin)?;
    refuses(
        NativeLoafClosure::admit(&native, &[root]),
        "sealed source, receipt or intent",
    )?;
    let payload = serde_json::to_vec(record)?;
    let mut receipt = record_receipt(record, &payload)?;
    receipt.intent.toolchain = "forged-toolchain".to_string();
    canonicalize_receipt(&mut receipt)?;
    let publication = native.publish(&OvenArtifactPublishRequest {
        receipt,
        domain: DOMAIN.to_string(),
        kind: OvenArtifactKind::Engine,
        payload,
        materialized_files: Vec::new(),
        materialized_directories: Vec::new(),
    })?;
    let mut root = graph
        .units
        .get(&identity)
        .ok_or("fixture missing")?
        .declared_root("fixture")?;
    root.record_identity = publication.identity;
    refuses(
        NativeLoafClosure::admit(&native, &[root]),
        "record payload, recipe, intent",
    )?;
    Ok(())
}

/// An explicitly dependency-free request is empty; a missing requested record cannot turn into that case.
#[test]
fn dev7_native_loaf_explicit_empty_closure_does_not_select_or_prepare() -> TestResult {
    let temp = tempfile::tempdir()?;
    let absent = temp.path().join("unused-store");
    let empty = NativeLoafClosure::admit(&store(&absent), &[])?;
    assert!(empty.roots().is_empty());
    assert!(empty.graph().units().is_empty());
    let _owners = empty.shared_owners()?;
    assert!(!absent.exists());
    let published = NativeLoafClosure::admit_published(&PublishedOvenStore::new(&absent), &[])?;
    assert!(published.graph().units().is_empty());
    assert!(!absent.exists());
    let mut graph = NativeLoafGraph::default();
    let native = store(&temp.path().join("populated-store"));
    let identity = publish(temp.path(), &native, &mut graph, "fixture", &[], &[])?;
    let unit = graph.units.get(&identity).ok_or("fixture missing")?;
    assert_eq!(
        unit.materialized_files()?
            .map(|file| file.relative_path.as_str())
            .collect::<Vec<_>>(),
        ["libfixture.rlib"]
    );
    assert!(graph.select(&[])?.graph().units().is_empty());
    let mut missing = unit.declared_root("missing")?;
    missing.record_identity = digest_bytes(b"not an admitted record");
    assert!(NativeLoafClosure::admit(&native, &[missing]).is_err());
    Ok(())
}
