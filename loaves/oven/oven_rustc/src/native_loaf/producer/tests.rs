//! Actual immutable Store fixtures for the bounded source/fact capability; fixture native bytes are never executed.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use oven_model::manifest::{RustFactCompileEnvironment, RustFactOut, RustFactRecord};
use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreLimits,
    PublishedOvenStore,
};
use oven_store::{OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project};

use super::super::{
    EDGES_INPUT, NativeLoafClosure, NativeLoafDependency, NativeLoafGraph, NativeLoafInspectionWork, NativeLoafOrigin,
    NativeLoafPhysicalBinding, NativeLoafReference, NativeLoafSource, ORIGIN_INPUT, SOURCE_INPUT, Store,
    physical_edges_input, select_owner, source_binding_input,
};
use crate::native_loaf::tests::replace_owned_fixture;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
const GENERATED: &[u8] = b"pub const GENERATED: bool = true;\n";

/// Explicit producer inputs; malformed selections are published independently, never smuggled through a hint.
struct Unit {
    name: String,
    version: String,
    role: String,
    domain: String,
    native_domain: String,
    root: String,
    origin: NativeLoafOrigin,
    environment: bool,
    root_inventory: bool,
    declaration_inventory: bool,
    fact: Option<RustFactRecord>,
    inputs: BTreeMap<String, Option<String>>,
}

impl Unit {
    /// Valid custom-root library input, with explicit selected compiler environment and local provenance.
    fn library(name: &str) -> Self {
        Self {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            role: "lib".to_string(),
            domain: "target".to_string(),
            native_domain: "ordinary-native-source-fixture".to_string(),
            root: "src/custom.rs".to_string(),
            origin: NativeLoafOrigin::Local,
            environment: true,
            root_inventory: true,
            declaration_inventory: true,
            fact: None,
            inputs: BTreeMap::new(),
        }
    }
}

/// Open a real bounded Store; all native/record/source owners fit without implicit external caches.
fn store(path: &Path) -> OvenStore {
    OvenStore::new(
        path,
        OvenStoreLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 16 * 1024 * 1024),
    )
}

/// Build a complete target/profile/feature-bound generated fact, including literal and relocatable environment.
fn fact() -> RustFactRecord {
    RustFactRecord {
        toolchain: "fixture-toolchain".to_string(),
        target: "fixture-target".to_string(),
        profile: "debug".to_string(),
        features: vec!["selected".to_string()],
        cfg: vec!["generated_cfg".to_string()],
        out: vec![RustFactOut {
            name: "generated.rs".to_string(),
            path: "facts/generated.rs".to_string(),
            digest: digest_bytes(GENERATED),
        }],
        environment: vec![
            RustFactCompileEnvironment {
                name: "GENERATED_INPUT".to_string(),
                literal: None,
                out: Some("generated.rs".to_string()),
            },
            RustFactCompileEnvironment {
                name: "EXACT_LITERAL".to_string(),
                literal: Some("/literal/is/not/rebased".to_string()),
                out: None,
            },
        ],
        link: Vec::new(),
        tool: Vec::new(),
        harvested_from: None,
    }
}

/// Publish the same genuine producer source/intent/physical bindings consumed by ordinary durable admission.
fn publish(
    root: &Path,
    store: &OvenStore,
    graph: &mut NativeLoafGraph,
    unit: &Unit,
    dependencies: &[(&str, &str)],
) -> TestResult<String> {
    let project = root.join(format!("publisher-{}", graph.units.len()));
    std::fs::create_dir_all(project.join("src"))?;
    let declaration = project.join("loaf.toml");
    let manifest: toml::Value = toml::from_str(&format!(
        "[project]\nname={:?}\nversion={:?}\n[rust]\nname='custom_crate'\ntype={:?}\nedition='2021'\n[rust.source]\nroot={:?}\n",
        unit.name, unit.version, unit.role, unit.root,
    ))?;
    std::fs::write(&declaration, toml::to_string(&manifest)?)?;
    let source_path = project.join("src/custom.rs");
    std::fs::write(&source_path, "pub const VALUE: u8 = 1;\n")?;
    let source = NativeLoafSource {
        loaf: unit.name.clone(),
        version: unit.version.clone(),
        archive_digest: digest_bytes(&std::fs::read(&declaration)?),
        domain: unit.domain.clone(),
        features: vec!["selected".to_string()],
        target_predicates: Vec::new(),
    };
    let environment = BTreeMap::from([
        ("CARGO_MANIFEST_DIR", project.to_string_lossy().into_owned()),
        (
            "CARGO_MANIFEST_PATH",
            project.join("Cargo.toml").to_string_lossy().into_owned(),
        ),
        ("CARGO_PKG_NAME", unit.name.clone()),
        ("CARGO_PKG_VERSION", unit.version.clone()),
        ("CARGO_CRATE_NAME", "custom_crate".to_string()),
        ("CARGO_PRIMARY_PACKAGE", "1".to_string()),
    ]);
    let mut inputs = BTreeMap::from([
        ("domain".to_string(), source.domain.clone()),
        (ORIGIN_INPUT.to_string(), unit.origin.as_str().to_string()),
        (SOURCE_INPUT.to_string(), source_binding_input(&source)?),
        ("compiler-host".to_string(), "fixture-host".to_string()),
        ("compiler-commit".to_string(), "fixture-commit".to_string()),
        ("compiler-binary".to_string(), digest_bytes(b"fixture sysroot")),
        (
            "native-compiler-executable".to_string(),
            digest_bytes(b"fixture compiler"),
        ),
        (
            "sdk-compile-policy".to_string(),
            "source-sealed-portable-v2".to_string(),
        ),
    ]);
    if unit.environment {
        inputs.insert(
            "sdk-compile-environment".to_string(),
            serde_json::to_string(&environment)?,
        );
    }
    if let Some(fact) = &unit.fact {
        inputs.insert("sdk-build-fact".to_string(), serde_json::to_string(fact)?);
    }
    let mut edges = Vec::new();
    for (alias, identity) in dependencies {
        let child = graph.units.get(*identity).ok_or("fixture dependency missing")?;
        inputs.insert(format!("extern:{alias}"), child.record.native.digest.clone());
        edges.push(NativeLoafDependency {
            alias: alias.to_string(),
            record_identity: identity.to_string(),
            native: child.record.native.clone(),
            source: child.record.source.clone(),
        });
    }
    let physical = edges
        .iter()
        .map(|edge| NativeLoafPhysicalBinding {
            alias: edge.alias.clone(),
            source: edge.source.clone(),
            native: edge.native.clone(),
        })
        .collect::<Vec<_>>();
    inputs.insert(EDGES_INPUT.to_string(), physical_edges_input(&physical)?);
    for (name, value) in &unit.inputs {
        match value {
            Some(value) => {
                inputs.insert(name.clone(), value.clone());
            }
            None => {
                inputs.remove(name);
            }
        }
    }
    let mut request = OvenGeneratedProjectRequest::new(
        &project,
        &source.loaf,
        &source.version,
        if unit.domain == "host" {
            "fixture-host"
        } else {
            "fixture-target"
        },
        "fixture-toolchain",
        "debug",
        source.features.clone(),
    )
    .with_generated_source("source-declaration", &declaration)
    .with_generated_source("source-root", &source_path);
    for (name, value) in inputs {
        request = request.with_build_unit_input(name, value);
    }
    let recipe = receipt_generated_project(&request)?;
    let output = project.join("libcustom.rlib");
    std::fs::write(&output, b"same native bytes, distinct source and producer owners")?;
    let mut files = vec![OvenArtifactMaterializedFile {
        source_path: output,
        relative_path: "libcustom.rlib".to_string(),
    }];
    if unit.declaration_inventory {
        files.push(OvenArtifactMaterializedFile {
            source_path: declaration,
            relative_path: "source/loaf.toml".to_string(),
        });
    }
    if unit.root_inventory {
        files.push(OvenArtifactMaterializedFile {
            source_path,
            relative_path: "source/src/custom.rs".to_string(),
        });
    }
    if unit.fact.is_some() {
        let generated = project.join("generated.rs");
        std::fs::write(&generated, GENERATED)?;
        files.push(OvenArtifactMaterializedFile {
            source_path: generated,
            relative_path: "source/.oven-out/generated.rs".to_string(),
        });
    }
    let published = store.publish(&OvenArtifactPublishRequest {
        receipt: recipe.clone(),
        domain: unit.native_domain.clone(),
        kind: OvenArtifactKind::Engine,
        payload: serde_json::to_vec(&source)?,
        materialized_files: files,
        materialized_directories: Vec::new(),
    })?;
    let native = NativeLoafReference {
        identity: published.identity.clone(),
        receipt_identity: recipe.identity.clone(),
        domain: published.domain,
        relative_path: "libcustom.rlib".to_string(),
        digest: published
            .materialized_files
            .iter()
            .find(|file| file.relative_path == "libcustom.rlib")
            .ok_or("native output missing")?
            .digest
            .clone(),
    };
    let owner = select_owner(Store::Writable(store), &published.identity)?;
    Ok(graph.publish(store, source, native, recipe, owner, edges)?)
}

/// Compare complete JSON and retained ownership through live and source-free published admission.
#[test]
fn dev7_native_source_projection_live_and_installed_preserve_exact_facts_and_aliases() -> TestResult {
    let root = tempfile::tempdir()?;
    let store = store(&root.path().join("store"));
    let mut graph = NativeLoafGraph::default();
    let mut child = Unit::library("crates-io/child");
    child.origin = NativeLoafOrigin::Registry;
    let child_id = publish(root.path(), &store, &mut graph, &child, &[])?;
    let mut parent = Unit::library("parent");
    parent.fact = Some(fact());
    let parent_id = publish(
        root.path(),
        &store,
        &mut graph,
        &parent,
        &[("renamed", &child_id), ("another_alias", &child_id)],
    )?;
    let selected = Arc::clone(graph.units.get(&parent_id).ok_or("parent missing")?);
    let native_owner = Arc::clone(&selected.native_owner);
    let roots = vec![selected.declared_root("public_parent")?];
    let closure = graph.select(&roots)?;
    let expected_work = NativeLoafInspectionWork {
        source_projection_attempts: 2,
        manifest_read_attempts: 2,
        generated_member_observation_attempts: 1,
        native_owner_acquisitions: 0,
    };
    let mut construction = NativeLoafInspectionWork::default();
    let inputs = closure.graph().inspection_inputs_with_work(&mut construction)?;
    assert_eq!(construction, expected_work);
    assert!(Arc::ptr_eq(
        inputs.units().get(&parent_id).ok_or("descriptor missing")?.selected(),
        &selected
    ));
    assert!(Arc::ptr_eq(
        &inputs
            .units()
            .get(&parent_id)
            .ok_or("descriptor missing")?
            .selected
            .native_owner,
        &native_owner
    ));
    let mut first_work = NativeLoafInspectionWork::default();
    let first = inputs.inspection_project_with_work(&mut first_work)?;
    assert_eq!(first_work, expected_work);
    let mut repeat_work = NativeLoafInspectionWork::default();
    assert_eq!(inputs.inspection_project_with_work(&mut repeat_work)?, first);
    assert_eq!(repeat_work, expected_work);
    let index = inputs
        .units()
        .keys()
        .position(|identity| identity == &parent_id)
        .ok_or("parent index missing")?;
    let child_index = inputs
        .units()
        .keys()
        .position(|identity| identity == &child_id)
        .ok_or("child index missing")?;
    let source = native_owner.artifact_root.join("source");
    let manifest_path = root
        .path()
        .join("publisher-1/Cargo.toml")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        first["crates"][index],
        serde_json::json!({
            "display_name":"custom_crate", "root_module":source.join("src/custom.rs"), "edition":"2021",
            "deps":[{"crate":child_index,"name":"another_alias"},{"crate":child_index,"name":"renamed"}],
            "cfg":["feature=\"selected\"","generated_cfg"],
            "env":{
                "CARGO_MANIFEST_DIR":source, "CARGO_MANIFEST_PATH":manifest_path,
                "CARGO_PKG_NAME":"parent", "CARGO_PKG_VERSION":"1.0.0", "CARGO_CRATE_NAME":"custom_crate", "CARGO_PRIMARY_PACKAGE":"1",
                "OUT_DIR":native_owner.artifact_root.join("source/.oven-out"),
                "GENERATED_INPUT":native_owner.artifact_root.join("source/.oven-out/generated.rs"), "EXACT_LITERAL":"/literal/is/not/rebased"
            }, "is_workspace_member":false,"is_proc_macro":false
        })
    );
    // Installed admission reads only the sealed owners; the original authored publication directories are absent.
    std::fs::remove_dir_all(root.path().join("publisher-0"))?;
    std::fs::remove_dir_all(root.path().join("publisher-1"))?;
    let durable =
        NativeLoafClosure::admit_published(&PublishedOvenStore::new(store.root()), &roots)?.inspection_inputs()?;
    assert_eq!(durable.scope_digest(), inputs.scope_digest());
    assert_eq!(durable.inspection_project()?, first);
    drop(graph);
    drop(closure);
    drop(selected);
    drop(native_owner);
    assert_eq!(inputs.inspection_project()?, first);
    let pressure = OvenStore::new(store.root(), OvenStoreLimits::new(1, 1, 1));
    assert!(pressure.prune()?.removed_entries.is_empty());
    Ok(())
}

/// Refuse independently sealed but incomplete or contradictory source/fact producer selections.
#[test]
fn dev7_native_source_projection_refuses_missing_compiler_environment_roots_and_bad_facts() -> TestResult {
    let mut invalid = Vec::new();
    let mut missing_env = Unit::library("missing_env");
    missing_env.environment = false;
    invalid.push(missing_env);
    let mut missing_compiler = Unit::library("missing_compiler");
    missing_compiler
        .inputs
        .insert("native-compiler-executable".to_string(), None);
    invalid.push(missing_compiler);
    let mut malformed = Unit::library("malformed_env");
    malformed
        .inputs
        .insert("sdk-compile-environment".to_string(), Some("[]".to_string()));
    invalid.push(malformed);
    let mut missing_manifest = Unit::library("missing_manifest");
    missing_manifest.declaration_inventory = false;
    invalid.push(missing_manifest);
    let mut missing_root = Unit::library("missing_root");
    missing_root.root_inventory = false;
    invalid.push(missing_root);
    let mut escape = Unit::library("escape");
    escape.root = "../custom.rs".to_string();
    invalid.push(escape);
    let mut unknown_role = Unit::library("unknown_role");
    unknown_role.role = "bin".to_string();
    invalid.push(unknown_role);
    let mut host_macro = Unit::library("macro_in_target");
    host_macro.role = "proc-macro".to_string();
    invalid.push(host_macro);
    let mut wrong_host = Unit::library("wrong_host");
    wrong_host.domain = "host".to_string();
    wrong_host
        .inputs
        .insert("compiler-host".to_string(), Some("other-host".to_string()));
    invalid.push(wrong_host);
    let mut wrong_fact = Unit::library("wrong_fact");
    let mut wrong = fact();
    wrong.features.clear();
    wrong_fact.fact = Some(wrong);
    invalid.push(wrong_fact);
    let mut wrong_digest = Unit::library("wrong_digest");
    let mut wrong = fact();
    wrong.out[0].digest = digest_bytes(b"other generated bytes");
    wrong_digest.fact = Some(wrong);
    invalid.push(wrong_digest);
    let mut bad_path = Unit::library("bad_path");
    let mut wrong = fact();
    wrong.environment[0].out = Some("../escape".to_string());
    bad_path.fact = Some(wrong);
    invalid.push(bad_path);
    for input in invalid {
        let root = tempfile::tempdir()?;
        let store = store(&root.path().join("store"));
        let mut graph = NativeLoafGraph::default();
        publish(root.path(), &store, &mut graph, &input, &[])?;
        assert!(
            graph.inspection_inputs().is_err(),
            "unexpected source admission for {}",
            input.name
        );
    }
    Ok(())
}

/// Detect preserved-mtime original member corruption at every handoff and accept exact restoration.
#[test]
fn dev7_native_source_projection_rechecks_original_sources_generated_bytes_and_recipe() -> TestResult {
    let root = tempfile::tempdir()?;
    let store = store(&root.path().join("store"));
    let mut graph = NativeLoafGraph::default();
    let mut input = Unit::library("retained");
    input.fact = Some(fact());
    let identity = publish(root.path(), &store, &mut graph, &input, &[])?;
    let inputs = graph.inspection_inputs()?;
    let expected = inputs.inspection_project()?;
    let selected = graph.units.get(&identity).ok_or("record missing")?;
    for member in [
        "source/loaf.toml",
        "source/src/custom.rs",
        "source/.oven-out/generated.rs",
    ] {
        let path = selected.native_owner.artifact_root.join(member);
        let bytes = std::fs::read(&path)?;
        let mut changed = bytes.clone();
        let byte = changed.first_mut().ok_or("empty source fixture")?;
        *byte ^= 1;
        replace_owned_fixture(&path, &changed)?;
        assert!(
            inputs.inspection_project().is_err(),
            "changed original member accepted: {member}"
        );
        replace_owned_fixture(&path, &bytes)?;
        assert_eq!(inputs.inspection_project()?, expected);
    }
    drop(inputs);
    let selected =
        Arc::get_mut(graph.units.get_mut(&identity).ok_or("record missing")?).ok_or("unexpected extra record lease")?;
    selected
        .record
        .recipe
        .sources
        .build_unit_inputs
        .insert("compiler-host".to_string(), "forged-host".to_string());
    assert!(graph.inspection_inputs().is_err());
    Ok(())
}

/// Keep distinct versions and host roles while enforcing the exact retained selected set.
#[test]
fn dev7_native_source_projection_keeps_versions_host_roles_and_exact_selected_set() -> TestResult {
    let root = tempfile::tempdir()?;
    let store = store(&root.path().join("store"));
    let mut graph = NativeLoafGraph::default();
    let one = publish(root.path(), &store, &mut graph, &Unit::library("versions"), &[])?;
    let mut second = Unit::library("versions");
    second.version = "2.0.0".to_string();
    let two = publish(root.path(), &store, &mut graph, &second, &[])?;
    let mut macro_unit = Unit::library("macro");
    macro_unit.role = "proc-macro".to_string();
    macro_unit.domain = "host".to_string();
    let macro_id = publish(root.path(), &store, &mut graph, &macro_unit, &[])?;
    publish(
        root.path(),
        &store,
        &mut graph,
        &Unit::library("consumer"),
        &[("old", &one), ("new", &two), ("derive", &macro_id)],
    )?;
    let projection = graph.inspection_inputs()?;
    assert!(
        projection
            .units()
            .get(&macro_id)
            .ok_or("macro descriptor missing")?
            .is_proc_macro()
    );
    assert_eq!(projection.units().len(), 4);
    projection.inspection_project()?;
    let root = graph.units.get(&one).ok_or("root missing")?.declared_root("one")?;
    let mut narrow_work = NativeLoafInspectionWork::default();
    let narrow = graph
        .select(&[root])?
        .graph()
        .inspection_inputs_with_work(&mut narrow_work)?;
    assert_eq!(
        narrow_work,
        NativeLoafInspectionWork {
            source_projection_attempts: 1,
            manifest_read_attempts: 1,
            generated_member_observation_attempts: 0,
            native_owner_acquisitions: 0,
        }
    );
    assert_eq!(narrow.units().len(), 1);
    assert_ne!(narrow.scope_digest(), projection.scope_digest());
    drop(projection);
    drop(narrow);
    graph.units.remove(&one);
    assert!(graph.inspection_inputs().is_err());
    let mut empty_work = NativeLoafInspectionWork::default();
    let empty = NativeLoafGraph::default().inspection_inputs_with_work(&mut empty_work)?;
    assert_eq!(
        empty.inspection_project_with_work(&mut empty_work)?,
        serde_json::json!({"crates":[]})
    );
    assert_eq!(empty_work, NativeLoafInspectionWork::default());
    Ok(())
}

/// Inspect transitional producer domains without creating writable closure proofs at construction or handoff.
#[test]
fn dev7_native_source_projection_is_read_only_even_for_transitional_native_domains() -> TestResult {
    let root = tempfile::tempdir()?;
    let store = store(&root.path().join("store"));
    let mut graph = NativeLoafGraph::default();
    let mut child = Unit::library("read_only_child");
    child.native_domain = "sdk-source-unit-target".to_string();
    let child_id = publish(root.path(), &store, &mut graph, &child, &[])?;
    let mut parent = Unit::library("read_only_parent");
    parent.native_domain = "sdk-source-unit-target".to_string();
    publish(root.path(), &store, &mut graph, &parent, &[("child", &child_id)])?;
    let proofs = store.root().join("closure-proofs");
    assert!(
        proofs.is_dir(),
        "fixture must exercise the writable native proof policy before inspection"
    );
    std::fs::remove_dir_all(&proofs)?;
    let inputs = graph.inspection_inputs()?;
    assert!(!proofs.exists());
    inputs.inspection_project()?;
    assert!(!proofs.exists());
    Ok(())
}

/// Count actual attempted boundaries on refusals without doing extra reads or acquiring native owners.
#[test]
fn dev7_native_source_projection_counters_preserve_refusal_boundaries() -> TestResult {
    let root = tempfile::tempdir()?;
    let store = store(&root.path().join("store"));
    let mut graph = NativeLoafGraph::default();
    let mut missing = Unit::library("missing_declaration");
    missing.declaration_inventory = false;
    publish(root.path(), &store, &mut graph, &missing, &[])?;
    let mut work = NativeLoafInspectionWork::default();
    assert!(graph.inspection_inputs_with_work(&mut work).is_err());
    assert_eq!(
        work,
        NativeLoafInspectionWork {
            source_projection_attempts: 1,
            manifest_read_attempts: 0,
            generated_member_observation_attempts: 0,
            native_owner_acquisitions: 0,
        }
    );

    let mut graph = NativeLoafGraph::default();
    let mut bad = Unit::library("bad_generated_digest");
    let mut selected_fact = fact();
    selected_fact.out[0].digest = digest_bytes(b"unrelated generated bytes");
    bad.fact = Some(selected_fact);
    publish(root.path(), &store, &mut graph, &bad, &[])?;
    let mut work = NativeLoafInspectionWork::default();
    assert!(graph.inspection_inputs_with_work(&mut work).is_err());
    assert_eq!(
        work,
        NativeLoafInspectionWork {
            source_projection_attempts: 1,
            manifest_read_attempts: 1,
            generated_member_observation_attempts: 1,
            native_owner_acquisitions: 0,
        }
    );
    Ok(())
}
