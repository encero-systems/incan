//! Actual prepared-consumer decision controls; fake compiler only answers metadata probes and never compiles.

use super::{
    Current, Hint, NativeLoafConsumerReport, NativeLoafConsumerRequest, SCHEMA, native_store,
    prepare_declared_native_loafs, prepare_with, write_hint,
};
use crate::native_loaf::preparation::NativeLoafPreparationReport;
use crate::native_loaf::tests::replace_owned_fixture;
use crate::native_loaf::{
    EDGES_INPUT, NativeLoafDependency, NativeLoafPhysicalBinding, NativeLoafReference, NativeLoafSource, ORIGIN_INPUT,
    SOURCE_INPUT, Store, physical_edges_input, select_owner, source_binding_input,
};
use crate::native_loaf::{NativeLoafGraph, NativeLoafOrigin, NativeLoafPreparation, refused};
use crate::sdk_closure::current_inputs;
use oven_model::manifest::{DependencySource, DependencySpec};
use oven_store::store::OvenStore;
use oven_store::store::{OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest};
use oven_store::{OvenGeneratedProjectRequest, receipt_generated_project};
use std::path::{Path, PathBuf};
use std::sync::Arc;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// A complete local source graph plus an executable metadata probe, without Cargo or native compilation.
struct Fixture {
    root: tempfile::TempDir,
    graph: PathBuf,
    rustc: PathBuf,
    index: PathBuf,
    output: PathBuf,
    dependencies: Vec<DependencySpec>,
}

impl Fixture {
    /// Create a genuine current local producer mapping and an explicit empty registry lock.
    fn new() -> TestResult<Self> {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir()?;
        let rustc = root.path().join("toolchain/bin/rustc");
        std::fs::create_dir_all(rustc.parent().ok_or("compiler parent missing")?)?;
        std::fs::write(
            &rustc,
            "#!/bin/sh\nif [ \"$1\" != '-vV' ]; then exit 91; fi\nprintf 'rustc 1.98.0 (fixture 2026-10-09)\\nhost: fixture-target\\ncommit-hash: fixture-commit\\n'\n",
        )?;
        std::fs::set_permissions(&rustc, std::fs::Permissions::from_mode(0o755))?;
        let sysroot = root.path().join("toolchain/lib/rustlib");
        std::fs::create_dir_all(&sysroot)?;
        std::fs::write(sysroot.join("fixture-std.rlib"), b"exact selected std")?;
        std::fs::write(
            sysroot.join("manifest-rust-std-fixture-target"),
            "file:lib/rustlib/fixture-std.rlib\n",
        )?;
        let index = root.path().join("index");
        std::fs::create_dir_all(&index)?;
        let graph = root.path().join("graph.json");
        std::fs::write(
            &graph,
            serde_json::to_vec(
                &serde_json::json!({"index_commit":"0000000000000000000000000000000000000000",
            "registry_lock":"lock.json", "facets":[{"project":"current", "features":[], "domain":"target"}]}),
            )?,
        )?;
        std::fs::write(
            root.path().join("lock.json"),
            r#"{"schema":"incan.oven.loaf-resolution/2","units":[]}"#,
        )?;
        std::fs::create_dir_all(root.path().join("current/src"))?;
        std::fs::write(root.path().join("current/src/lib.rs"), "pub const VALUE: u8 = 1;\n")?;
        std::fs::write(
            root.path().join("current/loaf.toml"),
            "[project]\nname='current'\nversion='1.0.0'\n[rust]\nname='current'\nedition='2021'\n",
        )?;
        let output = root.path().join("native");
        let dependencies = vec![DependencySpec {
            crate_name: "renamed".to_string(),
            version: Some("^1.0".to_string()),
            features: Vec::new(),
            default_features: true,
            optional: false,
            package: None,
            source: DependencySource::Path {
                path: root.path().join("current"),
            },
        }];
        Ok(Self {
            root,
            graph,
            rustc,
            index,
            output,
            dependencies,
        })
    }

    /// Pass all current authority inputs explicitly through the real public request shape.
    fn request(&self) -> NativeLoafConsumerRequest<'_> {
        NativeLoafConsumerRequest {
            graph: &self.graph,
            index: &self.index,
            blobs: &self.index,
            output: &self.output,
            rustc: &self.rustc,
            target: "fixture-target",
            profile: "debug",
            dependencies: &self.dependencies,
            declaration_owner: self.root.path(),
            domain: "target",
        }
    }

    /// Seal fixture native data with the exact current recipe inputs and genuine current local mapping.
    fn publish(
        &self,
        current: &Current,
        graph: &mut NativeLoafGraph,
        dependencies: &[(&str, &str)],
    ) -> TestResult<String> {
        let (manifest, digest) = crate::sdk_closure::local_native_source_selection(&self.root.path().join("current"))?;
        let source = NativeLoafSource {
            loaf: "current".to_string(),
            version: "1.0.0".to_string(),
            archive_digest: digest,
            domain: "target".to_string(),
            features: Vec::new(),
            target_predicates: Vec::new(),
        };
        publish_data(
            &self.output,
            &native_store(&self.output),
            graph,
            current,
            source,
            manifest,
            NativeLoafOrigin::Local,
            dependencies,
        )
    }
}

/// Publish exact current native data and immutable records; test outputs never invoke a native compiler.
fn publish_data(
    root: &Path,
    store: &OvenStore,
    graph: &mut NativeLoafGraph,
    current: &Current,
    source: NativeLoafSource,
    manifest: toml::Value,
    origin: NativeLoafOrigin,
    dependencies: &[(&str, &str)],
) -> TestResult<String> {
    let project = root.join(format!("fixture-{}", graph.units.len()));
    std::fs::create_dir_all(&project)?;
    let manifest_path = project.join("loaf.toml");
    std::fs::write(&manifest_path, toml::to_string(&manifest)?)?;
    let binding = serde_json::from_value(serde_json::to_value(&source)?)?;
    let environment = current_inputs::current_environment(&binding, &manifest, origin, &serde_json::Value::Null, None)?;
    let mut recipe = OvenGeneratedProjectRequest::new(
        &project,
        &source.loaf,
        &source.version,
        "fixture-target",
        &current.compiler.identity,
        "debug",
        source.features.clone(),
    )
    .with_generated_source("sdk-root", &manifest_path)
    .with_build_unit_input("domain", &source.domain)
    .with_build_unit_input("compiler-binary", &current.compiler.std)
    .with_build_unit_input("native-compiler-executable", &current.compiler.executable)
    .with_build_unit_input("compiler-host", &current.compiler.host)
    .with_build_unit_input("compiler-commit", &current.compiler.commit)
    .with_build_unit_input("sdk-compile-policy", "source-sealed-portable-v2")
    .with_build_unit_input("sdk-compile-environment", serde_json::to_string(&environment)?)
    .with_build_unit_input(ORIGIN_INPUT, origin.as_str())
    .with_build_unit_input(SOURCE_INPUT, source_binding_input(&source)?);
    let mut edges = Vec::new();
    for (alias, identity) in dependencies {
        let child = graph.units.get(*identity).ok_or("child missing")?;
        recipe = recipe.with_build_unit_input(format!("extern:{alias}"), &child.record.native.digest);
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
    let recipe =
        receipt_generated_project(&recipe.with_build_unit_input(EDGES_INPUT, physical_edges_input(&physical)?))?;
    let output = project.join("libfixture.rlib");
    std::fs::write(&output, b"identical fixture native bytes")?;
    let publication = store.publish(&OvenArtifactPublishRequest {
        receipt: recipe.clone(),
        domain: "ordinary-native-fixture".to_string(),
        kind: OvenArtifactKind::Engine,
        payload: serde_json::to_vec(&source)?,
        materialized_files: vec![
            OvenArtifactMaterializedFile {
                source_path: output,
                relative_path: "libfixture.rlib".to_string(),
            },
            OvenArtifactMaterializedFile {
                source_path: manifest_path,
                relative_path: "source/loaf.toml".to_string(),
            },
        ],
        materialized_directories: Vec::new(),
    })?;
    let native = NativeLoafReference {
        identity: publication.identity.clone(),
        receipt_identity: recipe.identity.clone(),
        domain: publication.domain,
        relative_path: "libfixture.rlib".to_string(),
        digest: publication
            .materialized_files
            .iter()
            .find(|file| file.relative_path == "libfixture.rlib")
            .ok_or("native member missing")?
            .digest
            .clone(),
    };
    let owner = select_owner(Store::Writable(store), &publication.identity)?;
    Ok(graph.publish(store, source, native, recipe, owner, edges)?)
}

/// Wrap already produced records at the actual preparation boundary without pretending fixture data was compiled.
fn prepared(graph: NativeLoafGraph) -> NativeLoafPreparation {
    NativeLoafPreparation {
        graph,
        observation: None,
        report: NativeLoafPreparationReport {
            compiled: Vec::new(),
            reused: Vec::new(),
            seconds: 0.0,
        },
    }
}

/// Unchanged commands admit only rooted original owners and never reach complete native preparation.
#[test]
fn dev7_native_loaf_prepared_repeat_skips_preparation_and_unrelated_owners() -> TestResult {
    let fixture = Fixture::new()?;
    let mut report = NativeLoafConsumerReport::default();
    let current = Current::read(&fixture.request(), &mut report)?;
    let mut graph = NativeLoafGraph::default();
    let identity = fixture.publish(&current, &mut graph, &[])?;
    let original = Arc::clone(graph.units.get(&identity).ok_or("original missing")?);
    let manifest: toml::Value =
        toml::from_str("[project]\nname='unrelated'\nversion='1.0.0'\n[rust]\nname='unrelated'\nedition='2021'\n")?;
    let unrelated = publish_data(
        &fixture.output,
        &native_store(&fixture.output),
        &mut graph,
        &current,
        NativeLoafSource {
            loaf: "unrelated".to_string(),
            version: "1.0.0".to_string(),
            archive_digest: "other-source".to_string(),
            domain: "target".to_string(),
            features: Vec::new(),
            target_predicates: Vec::new(),
        },
        manifest,
        NativeLoafOrigin::Local,
        &[],
    )?;
    let unrelated_path = graph.units.get(&unrelated).ok_or("unrelated missing")?.output()?;
    let first = prepare_with(&fixture.request(), || Ok(prepared(graph)))?;
    assert_eq!(first.report.preparation_calls, 1);
    assert_eq!(first.report.prepared_units, 2);
    assert_eq!(first.report.selected_units, 1);
    assert!(Arc::ptr_eq(
        &original,
        first.closure.graph.units.get(&identity).ok_or("selected missing")?
    ));
    replace_owned_fixture(&unrelated_path, b"corrupt unrelated owner must not be observed")?;
    let repeat = prepare_with(&fixture.request(), || Err(refused("unexpected full preparation")))?;
    assert!(repeat.report.prepared_reuse);
    assert_eq!(repeat.report.preparation_calls, 0);
    assert_eq!(repeat.report.prepared_units, 0);
    assert_eq!(repeat.report.selected_units, 1);
    assert_eq!(repeat.report.selected_native_owners, 1);
    assert_eq!(repeat.report.current_local_sources, 1);
    assert_eq!(repeat.report.current_registry_bindings, 0);
    assert_eq!(repeat.report.compiler_closure_checks, 1);
    assert!(repeat.report.compiled.is_empty());
    Ok(())
}

/// Preserved-mtime source/compiler edits invalidate the real reuse decision; restoration admits the old generation.
#[test]
fn dev7_native_loaf_prepared_source_compiler_edits_and_restoration() -> TestResult {
    let fixture = Fixture::new()?;
    let current = Current::read(&fixture.request(), &mut NativeLoafConsumerReport::default())?;
    let mut graph = NativeLoafGraph::default();
    fixture.publish(&current, &mut graph, &[])?;
    prepare_with(&fixture.request(), || Ok(prepared(graph)))?;
    for path in [
        fixture.root.path().join("current/src/lib.rs"),
        fixture.rustc.clone(),
        fixture.root.path().join("toolchain/lib/rustlib/fixture-std.rlib"),
    ] {
        let original = std::fs::read(&path)?;
        let modified = std::fs::metadata(&path)?.modified()?;
        let mut replacement = original.clone();
        if path == fixture.rustc {
            replacement.extend_from_slice(b"# changed executable\n");
        } else {
            replacement[0] ^= 1;
        }
        std::fs::write(&path, replacement)?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)?
            .set_times(std::fs::FileTimes::new().set_modified(modified))?;
        let error = prepare_with(&fixture.request(), || Err(refused("current input preparation miss")))
            .err()
            .ok_or("changed current input accepted")?;
        assert!(
            error.to_string().contains("preparation miss"),
            "unexpected error: {error}"
        );
        std::fs::write(&path, original)?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)?
            .set_times(std::fs::FileTimes::new().set_modified(modified))?;
        assert!(
            prepare_with(&fixture.request(), || Err(refused(
                "unexpected preparation after restoration"
            )))?
            .report
            .prepared_reuse
        );
    }
    Ok(())
}

/// Graph/lock/declaration edits cannot reuse previous roots, and forged empty roots cannot bypass required owners.
#[test]
fn dev7_native_loaf_prepared_graph_lock_declaration_and_hint_refusals() -> TestResult {
    let mut fixture = Fixture::new()?;
    let current = Current::read(&fixture.request(), &mut NativeLoafConsumerReport::default())?;
    let mut graph = NativeLoafGraph::default();
    fixture.publish(&current, &mut graph, &[])?;
    prepare_with(&fixture.request(), || Ok(prepared(graph)))?;
    for path in [&fixture.graph, &fixture.root.path().join("lock.json")] {
        let original = std::fs::read(path)?;
        let mut changed = original.clone();
        changed.extend_from_slice(b"\n ");
        std::fs::write(path, changed)?;
        let error = prepare_with(&fixture.request(), || Err(refused("current graph preparation miss")))
            .err()
            .ok_or("changed graph/lock key accepted")?;
        assert!(error.to_string().contains("preparation miss"));
        std::fs::write(path, original)?;
    }
    fixture.dependencies[0].features.push("missing".to_string());
    assert!(prepare_with(&fixture.request(), || Err(refused("declaration preparation miss"))).is_err());
    fixture.dependencies[0].features.clear();
    let hint_path = fixture
        .output
        .join("consumer-hints")
        .join(format!("{}.json", current.key.replace(':', "-")));
    write_hint(
        &hint_path,
        &Hint {
            schema: SCHEMA.to_string(),
            key: current.key,
            roots: Vec::new(),
        },
    )?;
    assert!(
        prepare_with(&fixture.request(), || Err(refused(
            "unexpected forged-hint preparation"
        )))
        .is_err()
    );
    Ok(())
}

/// Empty active declarations bypass compiler, graph, Store and archive inputs completely.
#[test]
fn dev7_native_loaf_prepared_empty_dependencies_bypass_all_inputs() -> TestResult {
    let root = tempfile::tempdir()?;
    let absent = root.path().join("absent");
    let request = NativeLoafConsumerRequest {
        graph: &absent,
        index: &absent,
        blobs: &absent,
        output: &absent,
        rustc: &absent,
        target: "unused",
        profile: "unused",
        dependencies: &[],
        declaration_owner: &absent,
        domain: "unused",
    };
    let prepared = prepare_declared_native_loafs(&request)?;
    assert!(prepared.closure.graph.units.is_empty());
    assert_eq!(prepared.report.preparation_calls, 0);
    assert_eq!(prepared.report.compiler_closure_checks, 0);
    assert!(!absent.exists());
    Ok(())
}

/// A forged new-key hint cannot hide another compatible local source candidate; incompatible features stay harmless.
#[test]
fn dev7_native_loaf_prepared_local_current_candidate_ambiguity_refuses() -> TestResult {
    let fixture = Fixture::new()?;
    let parent = fixture.root.path().join("current/loaf.toml");
    let mut declaration = std::fs::read_to_string(&parent)?;
    declaration.push_str("\n[dependencies]\nchild_alias={loaf='child',version='^1.0',features=['chosen']}\n");
    std::fs::write(&parent, declaration)?;
    for (directory, version) in [("child", "1.0.0"), ("alternative", "1.1.0")] {
        std::fs::create_dir_all(fixture.root.path().join(directory).join("src"))?;
        std::fs::write(
            fixture.root.path().join(directory).join("src/lib.rs"),
            "pub fn child() {}\n",
        )?;
        std::fs::write(
            fixture.root.path().join(directory).join("loaf.toml"),
            format!(
                "[project]\nname='child'\nversion='{version}'\n[project.features]\nchosen=[]\n[rust]\nname='child'\nedition='2021'\n"
            ),
        )?;
    }
    let mut input = serde_json::json!({"index_commit":"0000000000000000000000000000000000000000",
        "registry_lock":"lock.json", "facets":[
            {"project":"child", "features":["chosen"], "domain":"target"},
            {"project":"current", "features":[], "domain":"target"}]});
    std::fs::write(&fixture.graph, serde_json::to_vec(&input)?)?;
    let current = Current::read(&fixture.request(), &mut NativeLoafConsumerReport::default())?;
    let (manifest, digest) = crate::sdk_closure::local_native_source_selection(&fixture.root.path().join("child"))?;
    let mut graph = NativeLoafGraph::default();
    let child = publish_data(
        &fixture.output,
        &native_store(&fixture.output),
        &mut graph,
        &current,
        NativeLoafSource {
            loaf: "child".to_string(),
            version: "1.0.0".to_string(),
            archive_digest: digest,
            domain: "target".to_string(),
            features: vec!["chosen".to_string()],
            target_predicates: Vec::new(),
        },
        manifest,
        NativeLoafOrigin::Local,
        &[],
    )?;
    let parent = fixture.publish(&current, &mut graph, &[("child_alias", &child)])?;
    let roots = vec![
        graph
            .units
            .get(&parent)
            .ok_or("parent missing")?
            .declared_root("renamed")?,
    ];
    let first = prepare_with(&fixture.request(), || Ok(prepared(graph)))?;
    assert_eq!(first.report.selected_native_owners, 2);
    assert_eq!(first.report.current_local_dependency_candidates, 1);
    input["facets"]
        .as_array_mut()
        .ok_or("facets missing")?
        .push(serde_json::Value::Null);
    for compatible in [false, true] {
        input["facets"][2] = serde_json::json!({"project":"alternative",
            "features":if compatible { vec!["chosen"] } else { Vec::<&str>::new() }, "domain":"target"});
        std::fs::write(&fixture.graph, serde_json::to_vec(&input)?)?;
        let changed = Current::read(&fixture.request(), &mut NativeLoafConsumerReport::default())?;
        let path = fixture
            .output
            .join("consumer-hints")
            .join(format!("{}.json", changed.key.replace(':', "-")));
        write_hint(
            &path,
            &Hint {
                schema: SCHEMA.to_string(),
                key: changed.key,
                roots: roots.clone(),
            },
        )?;
        let reached = std::cell::Cell::new(false);
        let result = prepare_with(&fixture.request(), || {
            reached.set(true);
            Err(refused("must not prepare through current candidate ambiguity"))
        });
        assert!(!reached.get());
        if compatible {
            let error = result.err().ok_or("compatible current alternative accepted")?;
            assert!(
                error.to_string().contains("ambiguous selected bindings"),
                "unexpected {error}"
            );
        } else {
            let result = result?;
            assert!(result.report.prepared_reuse);
            assert_eq!(result.report.preparation_calls, 0);
            assert_eq!(result.report.prepared_units, 0);
            assert_eq!(result.report.selected_native_owners, 2);
            assert_eq!(result.report.current_local_sources, 2);
            assert_eq!(result.report.current_local_dependency_candidates, 2);
        }
    }
    input["facets"]
        .as_array_mut()
        .ok_or("facets missing")?
        .pop()
        .ok_or("alternative facet missing")?;
    std::fs::write(&fixture.graph, serde_json::to_vec(&input)?)?;
    assert!(
        prepare_with(&fixture.request(), || Err(refused(
            "unexpected restoration preparation"
        )))?
        .report
        .prepared_reuse
    );
    Ok(())
}

/// Invoke Git only inside the test-owned temporary index, preserving full diagnostics on failure.
fn git(root: &Path, arguments: &[&str]) -> TestResult<Vec<u8>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()?;
    if !output.status.success() {
        return Err(format!("fixture git {arguments:?}: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(output.stdout)
}

/// Current lock edges are independent authority even if a forged coordinate hint carries the new input key.
#[test]
fn dev7_native_loaf_prepared_registry_edges_features_and_corrupt_owner_refuse() -> TestResult {
    let mut fixture = Fixture::new()?;
    let mut sources = Vec::new();
    let mut manifests = Vec::new();
    std::fs::create_dir_all(fixture.index.join("index/crates-io"))?;
    for name in ["child", "parent"] {
        let source = NativeLoafSource {
            loaf: format!("crates-io/{name}"),
            version: "1.0.0".to_string(),
            archive_digest: oven_store::digest_bytes(name.as_bytes()),
            domain: "target".to_string(),
            features: Vec::new(),
            target_predicates: Vec::new(),
        };
        let manifest: toml::Value = toml::from_str(&format!(
            "[project]\nname='crates-io/{name}'\nversion='1.0.0'\n[rust]\nname='{name}_library'\nedition='2021'\n"
        ))?;
        let relative = format!("{name}.toml");
        std::fs::write(fixture.index.join(&relative), toml::to_string(&manifest)?)?;
        std::fs::write(
            fixture.index.join(format!("index/crates-io/{name}")),
            serde_json::to_vec(&serde_json::json!({
            "vers":"1.0.0", "cksum":source.archive_digest, "manifest":relative, "features":{}}))?,
        )?;
        sources.push(source);
        manifests.push(manifest);
    }
    git(&fixture.index, &["init", "--quiet"])?;
    git(&fixture.index, &["add", "."])?;
    git(
        &fixture.index,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "test native current index",
        ],
    )?;
    let commit = String::from_utf8(git(&fixture.index, &["rev-parse", "HEAD"])?)?
        .trim()
        .to_string();
    std::fs::write(
        &fixture.graph,
        serde_json::to_vec(&serde_json::json!({"index_commit":commit, "registry_lock":"lock.json", "facets":[]}))?,
    )?;
    let mut lock = serde_json::json!({"schema":"incan.oven.loaf-resolution/2", "units":[
        serde_json::to_value(&sources[0])?, serde_json::to_value(&sources[1])?]});
    lock["units"][0]["edges"] = serde_json::json!([]);
    lock["units"][1]["edges"] = serde_json::json!([
        {"dependency_key":"renamed_child", "loaf":"crates-io/child", "version":"1.0.0", "domain":"target"},
        {"dependency_key":"child", "loaf":"crates-io/child", "version":"1.0.0", "domain":"target"}]);
    std::fs::write(fixture.root.path().join("lock.json"), serde_json::to_vec(&lock)?)?;
    fixture.dependencies = vec![DependencySpec {
        crate_name: "parent".to_string(),
        version: Some("^1.0".to_string()),
        features: Vec::new(),
        default_features: true,
        optional: false,
        package: None,
        source: DependencySource::Registry,
    }];
    let current = Current::read(&fixture.request(), &mut NativeLoafConsumerReport::default())?;
    let store = native_store(&fixture.output);
    let mut graph = NativeLoafGraph::default();
    let child = publish_data(
        &fixture.output,
        &store,
        &mut graph,
        &current,
        sources[0].clone(),
        manifests[0].clone(),
        NativeLoafOrigin::Registry,
        &[],
    )?;
    let parent = publish_data(
        &fixture.output,
        &store,
        &mut graph,
        &current,
        sources[1].clone(),
        manifests[1].clone(),
        NativeLoafOrigin::Registry,
        &[("renamed_child", &child), ("child_library", &child)],
    )?;
    let corrupt_path = graph.units.get(&child).ok_or("child missing")?.output()?;
    let first = prepare_with(&fixture.request(), || Ok(prepared(graph)))?;
    assert_eq!(first.report.selected_units, 2);
    let roots = vec![
        first
            .closure
            .graph
            .units
            .get(&parent)
            .ok_or("parent missing")?
            .declared_root("parent")?,
    ];
    let repeat = prepare_with(&fixture.request(), || Err(refused("unexpected registry preparation")))?;
    assert!(repeat.report.prepared_reuse);
    assert_eq!(repeat.report.preparation_calls, 0);
    assert_eq!(repeat.report.current_registry_bindings, 2);
    // Actual current-recipe replay reads four distinct pinned blobs, independent of duplicate physical aliases.
    for report in [first.report(), repeat.report()] {
        assert_eq!(report.index_file_requests, 4);
        assert_eq!(report.index_blob_reads, 4);
        assert_eq!(report.index_blob_cache_hits, 0);
        assert!(report.index_blob_bytes > 0);
        if report.index_batch_requests > 0 {
            assert_eq!(report.index_batch_requests, 5); // Commit plus four raw files.
            assert_eq!(report.index_git_processes, 3); // Capability, persistent child, event listing.
        } else {
            assert_eq!(report.index_git_processes, 11); // Capability, admission, eight file processes, listing.
        }
    }
    for change in ["edges", "features"] {
        let mut changed = lock.clone();
        if change == "edges" {
            changed["units"][1]["edges"][0]["dependency_key"] = serde_json::json!("forged_alias");
        } else {
            changed["units"][0]["features"] = serde_json::json!(["unselected"]);
        }
        std::fs::write(fixture.root.path().join("lock.json"), serde_json::to_vec(&changed)?)?;
        let changed_current = Current::read(&fixture.request(), &mut NativeLoafConsumerReport::default())?;
        let path = fixture
            .output
            .join("consumer-hints")
            .join(format!("{}.json", changed_current.key.replace(':', "-")));
        write_hint(
            &path,
            &Hint {
                schema: SCHEMA.to_string(),
                key: changed_current.key,
                roots: roots.clone(),
            },
        )?;
        let error = prepare_with(&fixture.request(), || Err(refused("forged lock requires preparation")))
            .err()
            .ok_or("forged current lock was accepted by a hint")?;
        assert!(
            error.to_string().contains("requires preparation"),
            "unexpected refusal: {error}"
        );
    }
    std::fs::write(fixture.root.path().join("lock.json"), serde_json::to_vec(&lock)?)?;
    let original_graph = std::fs::read(&fixture.graph)?;
    for change in ["fact", "about"] {
        let mut manifest = toml::to_string(&manifests[1])?;
        if change == "fact" {
            manifest.push_str(&format!(
                "\n[[rust.facts]]\ntoolchain={:?}\ntarget='fixture-target'\nprofile='debug'\ncfg=['changed']\n",
                current.compiler.identity
            ));
        } else {
            std::fs::create_dir_all(fixture.index.join("events"))?;
            std::fs::write(
                fixture.index.join("events/000001-adopt-crates-io-parent-1.0.0.json"),
                serde_json::to_vec(&serde_json::json!({
                "kind":"adopt", "archive":sources[1].archive_digest, "about":{"description":"changed metadata"}}))?,
            )?;
        }
        std::fs::write(fixture.index.join("parent.toml"), manifest)?;
        git(&fixture.index, &["add", "."])?;
        git(
            &fixture.index,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "test current authority change",
            ],
        )?;
        let revision = String::from_utf8(git(&fixture.index, &["rev-parse", "HEAD"])?)?
            .trim()
            .to_string();
        std::fs::write(
            &fixture.graph,
            serde_json::to_vec(
                &serde_json::json!({"index_commit":revision, "registry_lock":"lock.json", "facets":[]}),
            )?,
        )?;
        let changed = Current::read(&fixture.request(), &mut NativeLoafConsumerReport::default())?;
        let path = fixture
            .output
            .join("consumer-hints")
            .join(format!("{}.json", changed.key.replace(':', "-")));
        write_hint(
            &path,
            &Hint {
                schema: SCHEMA.to_string(),
                key: changed.key,
                roots: roots.clone(),
            },
        )?;
        let error = prepare_with(&fixture.request(), || {
            Err(refused("current index authority requires preparation"))
        })
        .err()
        .ok_or("changed pinned fact/about authority accepted")?;
        assert!(
            error.to_string().contains("requires preparation"),
            "unexpected {change} refusal: {error}"
        );
    }
    std::fs::write(&fixture.graph, original_graph)?;
    replace_owned_fixture(&corrupt_path, b"corrupt selected native owner")?;
    let reached = std::cell::Cell::new(false);
    assert!(
        prepare_with(&fixture.request(), || {
            reached.set(true);
            Err(refused("must not prepare through corruption"))
        })
        .is_err()
    );
    assert!(!reached.get());
    Ok(())
}

/// Map-derived declaration permutations share one real hint and physical owner; constraints and alias refusals remain
/// exact.
#[test]
fn dev7_native_loaf_prepared_declaration_permutations_reuse() -> TestResult {
    let mut fixture = Fixture::new()?;
    let declaration = fixture.dependencies[0].clone();
    fixture.dependencies = ["z-last", "a_first", "middle"]
        .iter()
        .map(|alias| DependencySpec {
            crate_name: alias.to_string(),
            ..declaration.clone()
        })
        .collect();
    let current = Current::read(&fixture.request(), &mut NativeLoafConsumerReport::default())?;
    let key = current.key.clone();
    let mut graph = NativeLoafGraph::default();
    fixture.publish(&current, &mut graph, &[])?;
    let selected = graph.select_dependency_roots(&fixture.dependencies, fixture.root.path(), "target")?;
    assert_eq!(
        selected.iter().map(|root| root.alias.as_str()).collect::<Vec<_>>(),
        ["a_first", "middle", "z_last"]
    );
    let first = prepare_with(&fixture.request(), || Ok(prepared(graph)))?;
    let aliases = first.closure.roots().keys().map(String::as_str).collect::<Vec<_>>();
    assert_eq!(aliases, ["a_first", "middle", "z_last"]);
    let dependencies = fixture.dependencies.clone();
    for order in [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]] {
        fixture.dependencies = order.into_iter().map(|index| dependencies[index].clone()).collect();
        let permuted = Current::read(&fixture.request(), &mut NativeLoafConsumerReport::default())?;
        assert_eq!(permuted.key, key);
        let repeat = prepare_with(&fixture.request(), || Err(refused("permutation unexpectedly prepared")))?;
        assert!(repeat.report.prepared_reuse);
        assert_eq!(repeat.report.preparation_calls, 0);
        assert_eq!(repeat.report.prepared_units, 0);
        assert_eq!(repeat.report.selected_units, 1);
        assert_eq!(repeat.report.selected_native_owners, 1);
        assert_eq!(repeat.closure.roots(), first.closure.roots());
    }
    assert_eq!(std::fs::read_dir(fixture.output.join("consumer-hints"))?.count(), 1);
    fixture.dependencies = dependencies.clone();
    let other = fixture.root.path().join("alternative");
    std::fs::create_dir(&other)?;
    for constraint in [
        "version", "features", "defaults", "optional", "package", "source", "alias",
    ] {
        fixture.dependencies = dependencies.clone();
        let dependency = &mut fixture.dependencies[0];
        match constraint {
            "version" => dependency.version = Some("^1.0.0".to_string()),
            "features" => dependency.features.push("new-feature".to_string()),
            "defaults" => dependency.default_features = !dependency.default_features,
            "optional" => dependency.optional = !dependency.optional,
            "package" => dependency.package = Some("current".to_string()),
            "source" => dependency.source = DependencySource::Path { path: other.clone() },
            "alias" => dependency.crate_name = "different".to_string(),
            _ => return Err("unknown declaration constraint".into()),
        }
        let changed = Current::read(&fixture.request(), &mut NativeLoafConsumerReport::default())?;
        assert_ne!(changed.key, key, "declaration constraint omitted: {constraint}");
    }
    fixture.dependencies = dependencies;
    let mut duplicate = fixture.dependencies[0].clone();
    duplicate.crate_name = "z_last".to_string();
    fixture.dependencies.push(duplicate);
    let duplicate_result = Current::read(&fixture.request(), &mut NativeLoafConsumerReport::default());
    assert!(duplicate_result.is_err());
    assert!(
        first
            .closure
            .graph
            .select_dependency_roots(&fixture.dependencies, fixture.root.path(), "target")
            .is_err()
    );
    Ok(())
}
