//! Real producer controls for exact native edge capture and unchanged compilation reuse.

use super::*;
use crate::sdk_closure::{
    CompileContext, SdkClosureReport, SdkCompiledClosure, compile_units, compiler_closure_digest,
};
use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreLimits,
};
use oven_store::{OvenGeneratedProjectRequest, receipt_generated_project};

/// Publish a real immutable Store owner and reproduced recipe without invoking a native compiler.
fn node(
    root: &Path,
    store: &OvenStore,
    name: &str,
    features: &[&str],
    selected: &[(&str, &SdkCompiledUnit)],
) -> Result<SdkCompiledUnit, Error> {
    let project = root.join(name);
    std::fs::create_dir_all(&project)?;
    let source = project.join("lib.rs");
    std::fs::write(&source, format!("pub const OWNER: &str = {name:?};\n"))?;
    let binding = SdkLockedUnit {
        loaf: format!("crates-io/{name}"),
        version: "1.0.0".to_string(),
        archive_digest: oven_store::digest_bytes(&std::fs::read(&source)?),
        domain: "target".to_string(),
        features: features.iter().map(|feature| feature.to_string()).collect(),
        target_predicates: Vec::new(),
        edges: None,
    };
    let mut request = OvenGeneratedProjectRequest::new(
        &project,
        &binding.loaf,
        &binding.version,
        "fixture-target",
        "fixture-compiler",
        "debug",
        binding.features.clone(),
    )
    .with_generated_source("sdk-root", &source)
    .with_build_unit_input("sdk-source-archive", &binding.archive_digest)
    .with_build_unit_input("domain", &binding.domain);
    request = request.with_build_unit_input(crate::native_loaf::ORIGIN_INPUT, "registry");
    for (alias, dependency) in selected {
        request = request.with_build_unit_input(format!("extern:{alias}"), dependency.native_artifact()?.digest);
    }
    request = request
        .with_build_unit_input(
            crate::native_loaf::SOURCE_INPUT,
            crate::native_loaf::source_binding_input(&super::super::ordinary_native_source(&binding))?,
        )
        .with_build_unit_input(
            crate::native_loaf::EDGES_INPUT,
            crate::native_loaf::physical_edges_input(&super::super::selected_native_bindings(selected)?)?,
        );
    let receipt = receipt_generated_project(&request)?;
    let output = project.join("libsame.rlib");
    std::fs::write(&output, b"byte-identical fixture native output")?;
    let publication = store.publish(&OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: format!("sdk-source-unit-{}", binding.domain),
        kind: OvenArtifactKind::Engine,
        payload: serde_json::to_vec(&binding.identity_binding())?,
        materialized_files: vec![OvenArtifactMaterializedFile {
            source_path: output,
            relative_path: "libsame.rlib".to_string(),
        }],
        materialized_directories: Vec::new(),
    })?;
    let owner = store
        .select_payloads_for_execution(&[publication.identity])?
        .pop()
        .ok_or("fixture owner missing")?;
    let physical_edges = capture(&owner, &receipt, selected)?;
    Ok(SdkCompiledUnit {
        binding,
        output: owner.artifact_root.join("libsame.rlib"),
        owner,
        inspection: serde_json::Value::Null,
        physical_edges,
        reproduced_receipt: receipt,
    })
}

/// Construct an isolated Store whose small capacity is sufficient for the control's immutable records.
fn store(root: &Path) -> OvenStore {
    OvenStore::new(
        root,
        OvenStoreLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 16 * 1024 * 1024),
    )
}

/// Assert one stable error family without panic helpers or treating an unrelated error as successful rejection.
fn refuses(result: Result<(), Error>, family: &str) -> Result<(), Error> {
    let error = result.err().ok_or("malformed physical edge was accepted")?;
    assert!(error.to_string().contains(family), "expected {family:?}, got {error}");
    Ok(())
}

/// Renamed aliases retain the selected full owner, and true empty leaves are proven by their reproduced recipes.
#[test]
fn dev7_physical_edges_capture_original_named_owner_without_reacquisition() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let native = store(&root.path().join("store"));
    let child = node(root.path(), &native, "child", &["enabled"], &[])?;
    let selected = [("renamed_child", &child)];
    let parent = node(root.path(), &native, "parent", &[], &selected)?;
    assert!(child.physical_edges().is_empty());
    let before = child.compiled_identity().to_string();
    let edges = parent.physical_edges();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].alias(), "renamed_child");
    assert_eq!(edges[0].destination().binding.features, ["enabled"]);
    assert_eq!(edges[0].destination().store_identity, child.entry_identity());
    assert_eq!(edges[0].destination().receipt_identity, before);
    assert_eq!(edges[0].store(), root.path().join("store").canonicalize()?);
    verify(&parent.owner, &parent.reproduced_receipt, edges, &selected)?;
    assert_eq!(child.compiled_identity(), before);
    // The fixture outputs are data controls; validation uses their descriptors, not rustc or a fresh owner search.
    assert_eq!(
        serde_json::to_value(capture(&parent.owner, &parent.reproduced_receipt, &selected)?)?,
        serde_json::to_value(edges)?
    );
    Ok(())
}

/// Byte-identical artifacts never substitute a different source, feature, receipt or Store owner.
#[test]
fn dev7_physical_edges_refuse_identical_output_different_owner() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let native = store(&root.path().join("store"));
    let selected = node(root.path(), &native, "selected", &["selected_feature"], &[])?;
    let other = node(root.path(), &native, "other", &["other_feature"], &[])?;
    assert_eq!(selected.native_artifact()?.digest, other.native_artifact()?.digest);
    assert_ne!(selected.entry_identity(), other.entry_identity());
    let expected = [("renamed", &selected)];
    let parent = node(root.path(), &native, "parent", &[], &expected)?;
    let substituted = vec![record("renamed", &other)?];
    refuses(
        verify(&parent.owner, &parent.reproduced_receipt, &substituted, &expected),
        "authoritative preparation destination",
    )?;
    verify(
        &parent.owner,
        &parent.reproduced_receipt,
        parent.physical_edges(),
        &expected,
    )?;
    Ok(())
}

/// Independent descriptor edits refuse before matching native digests; absent and injected aliases refuse receipts.
#[test]
fn dev7_physical_edges_refuse_corrupted_binding_and_extern_authority() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let native = store(&root.path().join("store"));
    let child = node(root.path(), &native, "child", &["required"], &[])?;
    let selected = [("renamed", &child)];
    let parent = node(root.path(), &native, "parent", &[], &selected)?;
    for case in 0..9 {
        let mut edges = parent.physical_edges().to_vec();
        let edge = edges.first_mut().ok_or("fixture edge missing")?;
        match case {
            0 => edge.destination.binding.archive_digest = oven_store::digest_bytes(b"wrong source"),
            1 => edge.destination.binding.features.clear(),
            2 => edge.destination.binding.domain = "host".to_string(),
            3 => edge.destination.binding.version = "2.0.0".to_string(),
            4 => edge.destination.store_identity = "substituted owner".to_string(),
            5 => edge.destination.receipt_identity = "substituted receipt".to_string(),
            6 => edge.destination.relative_path = "different.rlib".to_string(),
            7 => edge.store = root.path().join("other-store"),
            _ => edge.destination.digest = oven_store::digest_bytes(b"wrong output"),
        }
        refuses(
            verify(&parent.owner, &parent.reproduced_receipt, &edges, &selected),
            "authoritative preparation destination",
        )?;
    }
    refuses(
        capture(&parent.owner, &parent.reproduced_receipt, &[]).map(|_| ()),
        "reproduced extern recipe",
    )?;
    refuses(
        capture(&parent.owner, &parent.reproduced_receipt, &[("injected", &child)]).map(|_| ()),
        "reproduced extern recipe",
    )?;
    refuses(
        capture(
            &parent.owner,
            &parent.reproduced_receipt,
            &[("renamed", &child), ("renamed", &child)],
        )
        .map(|_| ()),
        "duplicate extern alias",
    )?;
    verify(
        &parent.owner,
        &parent.reproduced_receipt,
        parent.physical_edges(),
        &selected,
    )?;
    Ok(())
}

/// A preparation node must reproduce its own admitted source binding, not merely a receipt's output digest.
#[test]
fn dev7_physical_edges_refuse_mutated_preparation_source_binding() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let native = store(&root.path().join("store"));
    let mut child = node(root.path(), &native, "child", &["required"], &[])?;
    let original = child.binding.clone();
    for case in 0..5 {
        child.binding = original.clone();
        match case {
            0 => child.binding.archive_digest = oven_store::digest_bytes(b"different source"),
            1 => child.binding.features.clear(),
            2 => child.binding.domain = "host".to_string(),
            3 => child.binding.version = "2.0.0".to_string(),
            _ => child
                .binding
                .target_predicates
                .push(crate::sdk_closure::SdkLockedPredicate {
                    declaration: 0,
                    target: "cfg(unselected)".to_string(),
                    matches: true,
                }),
        }
        refuses(record("renamed", &child).map(|_| ()), "admitted source binding")?;
    }
    child.binding = original;
    record("renamed", &child)?;
    Ok(())
}

/// Unsealed descriptor edges cannot inject dependencies into records taken from actual preparation nodes.
#[test]
fn dev7_physical_edges_ignore_unsealed_descriptor_edges() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let native = store(&root.path().join("store"));
    let mut child = node(root.path(), &native, "child", &[], &[])?;
    let original = record("renamed", &child)?;
    child.binding.edges = Some(vec![crate::sdk_closure::SdkLockedEdge {
        dependency_key: "injected".to_string(),
        loaf: "crates-io/unselected".to_string(),
        version: "99.0.0".to_string(),
        domain: "host".to_string(),
    }]);
    let actual = record("renamed", &child)?;
    assert_eq!(serde_json::to_value(original)?, serde_json::to_value(actual)?);
    assert!(child.physical_edges().is_empty());
    Ok(())
}

/// Engine owners have no on-disk recipe; only an exact canonical reproduced recipe can establish edges.
#[test]
fn dev7_physical_edges_refuse_missing_or_substituted_recipe() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let native = store(&root.path().join("store"));
    let leaf = node(root.path(), &native, "leaf", &[], &[])?;
    let other = node(root.path(), &native, "other", &[], &[])?;
    assert!(leaf.owner.original_native_receipt().is_none());
    verify_recipe(&leaf.owner, &leaf.reproduced_receipt, &[])?;
    refuses(
        verify_recipe(&leaf.owner, &other.reproduced_receipt, &[]),
        "selected Engine owner",
    )?;
    let mut incomplete = leaf.reproduced_receipt.clone();
    incomplete.sources.build_unit_inputs.clear();
    // Missing recipe facts cannot be silently interpreted as an authenticated empty closure.
    refuses(
        verify_recipe(&leaf.owner, &incomplete, &[]),
        "Oven receipt identity mismatch",
    )?;
    let mut wrong_build_unit = leaf.reproduced_receipt.clone();
    wrong_build_unit.build_unit_identity = oven_store::digest_bytes(b"wrong build unit");
    refuses(
        verify_recipe(&leaf.owner, &wrong_build_unit, &[]),
        "Oven build-unit identity mismatch",
    )?;
    let changed = oven_store::receipt_with_build_unit_input(&leaf.reproduced_receipt, "extern:injected", "wrong")?;
    refuses(verify_recipe(&leaf.owner, &changed, &[]), "selected Engine owner")?;
    Ok(())
}

/// Real local compilation and both persisted/in-memory reuse keep edge metadata without introducing new recipes.
#[test]
fn dev7_physical_edges_local_first_and_repeat_preserve_work_and_identity() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let output = root.path().join("output");
    for (name, source) in [
        ("leaf", "pub fn value() -> u8 { 42 }"),
        ("parent", "pub fn value() -> u8 { renamed::value() }"),
    ] {
        let project = root.path().join(name);
        std::fs::create_dir_all(project.join("src"))?;
        std::fs::write(project.join("src/lib.rs"), source)?;
        let dependencies = if name == "parent" {
            "[dependencies]\nrenamed={loaf='leaf',path='../leaf'}\n"
        } else {
            ""
        };
        std::fs::write(
            project.join("loaf.toml"),
            format!(
                "[project]\nname='{name}'\nversion='1.0.0'\n[rust]\nname='{name}'\ntype='lib'\nedition='2024'\n{dependencies}"
            ),
        )?;
    }
    let rustc = crate::rustc::resolve_active_rustc()?;
    let mut identities = Vec::new();
    for repeat in 0..2 {
        let mut closure = SdkCompiledClosure {
            report: SdkClosureReport::default(),
            units: Vec::new(),
            auxiliary_targets: BTreeMap::new(),
        };
        for name in ["leaf", "parent"] {
            crate::sdk_closure::compile_local_sdk_facet(
                &mut closure,
                &root.path().join(name),
                &[],
                "target",
                &output,
                &rustc,
            )?;
        }
        assert_eq!(closure.report.compiled.len() + closure.report.reused.len(), 2);
        if repeat > 0 {
            assert!(closure.report.compiled.is_empty());
            assert_eq!(closure.report.reused.len(), 2);
        }
        let compiled = closure.report.compiled.len();
        let reused = closure.report.reused.len();
        let parent = closure.units.get(1).ok_or("compiled parent missing")?;
        assert_eq!(parent.physical_edges()[0].alias(), "renamed");
        assert_eq!(
            parent.physical_edges()[0].destination().store_identity,
            closure.units[0].entry_identity()
        );
        identities.push((
            parent.entry_identity().to_string(),
            parent.compiled_identity().to_string(),
            serde_json::to_value(parent.physical_edges())?,
        ));
        crate::sdk_closure::compile_local_sdk_facet(
            &mut closure,
            &root.path().join("parent"),
            &[],
            "target",
            &output,
            &rustc,
        )?;
        assert_eq!(closure.report.compiled.len(), compiled);
        assert_eq!(closure.report.reused.len(), reused + 1);
    }
    assert_eq!(identities[0], identities[1]);
    Ok(())
}

/// Registry preparation retains actual lock-selected alias destinations identically on compiled and reused paths.
#[test]
fn dev7_physical_edges_registry_first_and_repeat_preserve_work_and_identity() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let rustc = crate::rustc::resolve_active_rustc()?;
    let target = crate::rustc::rustc_host_target(&rustc)?;
    let toolchain = crate::rustc::rustc_identity(&rustc)?;
    let native = store(&root.path().join("store"));
    let mut units = Vec::new();
    for (name, source) in [
        ("leaf", "pub fn value() -> u8 { 42 }"),
        ("parent", "pub fn value() -> u8 { renamed::value() }"),
    ] {
        let project = root.path().join(name);
        std::fs::create_dir_all(project.join("src"))?;
        std::fs::write(project.join("src/lib.rs"), source)?;
        let manifest = format!(
            "[project]\nname='crates-io/{name}'\nversion='1.0.0'\n[rust]\nname='{name}'\ntype='lib'\nedition='2024'\n"
        );
        std::fs::write(project.join("loaf.toml"), &manifest)?;
        let mut unit = crate::sdk_closure::tests::unit(&format!("crates-io/{name}"), "target", &manifest, &[])?;
        unit.root = project;
        unit.binding.archive_digest = oven_store::digest_source_tree(&unit.root)?;
        unit.binding.edges = Some(if name == "parent" {
            vec![crate::sdk_closure::SdkLockedEdge {
                dependency_key: "renamed".to_string(),
                loaf: "crates-io/leaf".to_string(),
                version: "1.0.0".to_string(),
                domain: "target".to_string(),
            }]
        } else {
            Vec::new()
        });
        units.push(unit);
    }
    let context = CompileContext {
        rustc: &rustc,
        target: &target,
        toolchain: &toolchain,
        output: root.path(),
        store: &native,
        compiler_digest: compiler_closure_digest(&rustc, &target)?,
        profile: "debug",
        unit_codegen: &[],
    };
    let first = compile_units(&units, &context)?;
    first.require_complete()?;
    assert_eq!(first.report.compiled.len() + first.report.reused.len(), 2);
    let repeat = compile_units(&units, &context)?;
    repeat.require_complete()?;
    assert!(repeat.report.compiled.is_empty());
    assert_eq!(repeat.report.reused.len(), 2);
    let parent = first.units.get(1).ok_or("registry parent missing")?;
    let repeated = repeat.units.get(1).ok_or("repeat registry parent missing")?;
    assert_eq!(parent.physical_edges()[0].alias(), "renamed");
    assert_eq!(
        parent.physical_edges()[0].destination().store_identity,
        first.units[0].entry_identity()
    );
    assert_eq!(parent.entry_identity(), repeated.entry_identity());
    assert_eq!(parent.compiled_identity(), repeated.compiled_identity());
    assert_eq!(
        serde_json::to_value(parent.physical_edges())?,
        serde_json::to_value(repeated.physical_edges())?
    );
    Ok(())
}

/// Producer-captured aliases become installed ordinary records; unauthenticated catalog edges never select nodes.
#[test]
fn dev7_native_loaf_producer_bridge_seals_exact_physical_generation() -> Result<(), Error> {
    use crate::native_loaf::{NativeLoafClosure, NativeLoafRoot};
    use oven_store::store::PublishedOvenStore;
    let root = tempfile::tempdir()?;
    let native = store(&root.path().join("store"));
    let child = node(root.path(), &native, "child", &["test_support"], &[])?;
    let child_native = child.entry_identity().to_string();
    let mut parent = node(root.path(), &native, "parent", &[], &[("actual_renamed_child", &child)])?;
    // Catalog-only edges are deliberately contradicted without changing the real producer-owned edge capability.
    parent.binding.edges = Some(vec![super::super::SdkLockedEdge {
        dependency_key: "catalog_injection".to_string(),
        loaf: "unrelated".to_string(),
        version: "99.0.0".to_string(),
        domain: "host".to_string(),
    }]);
    let unrelated = node(root.path(), &native, "unrelated", &[], &[])?;
    let parent_native = parent.entry_identity().to_string();
    let closure = SdkCompiledClosure {
        report: SdkClosureReport::default(),
        units: vec![child, parent, unrelated],
        auxiliary_targets: BTreeMap::new(),
    };
    let graph = closure.into_native_loafs(&native)?;
    let parent = graph
        .units()
        .values()
        .find(|unit| unit.record().native.identity == parent_native)
        .ok_or("parent record missing")?;
    let selected_root = NativeLoafRoot {
        alias: "authored_parent".to_string(),
        record_identity: parent.identity().to_string(),
        source: parent.record().source.clone(),
        intent: parent.record().recipe.intent.clone(),
    };
    let selected = graph.select(&[selected_root.clone()])?;
    assert_eq!(selected.graph().units().len(), 2);
    assert_eq!(parent.record().dependencies.len(), 1);
    assert_eq!(parent.record().dependencies[0].alias, "actual_renamed_child");
    assert_eq!(parent.record().dependencies[0].native.identity, child_native);
    drop(graph);
    let installed = NativeLoafClosure::admit_published(&PublishedOvenStore::new(native.root()), &[selected_root])?;
    assert_eq!(
        installed.graph().units().keys().collect::<Vec<_>>(),
        selected.graph().units().keys().collect::<Vec<_>>()
    );
    Ok(())
}
