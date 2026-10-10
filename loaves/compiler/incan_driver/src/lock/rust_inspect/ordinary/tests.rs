//! Genuine native publication and Rust metadata controls for ordinary retained inspection.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use incan_provider::dependency_resolver::ResolvedDependencies;
use incan_provider::requirements::ProjectRequirements;
use oven_model::manifest::{DependencySource, DependencySpec};
use oven_rustc::native_loaf::{NativeLoafFacet, NativeLoafPreparationRequest, prepare_native_loafs};
use oven_rustc::rustc::{resolve_active_rustc, rustc_host_target, rustc_identity};
use rust_inspect::{RustMetadataCache, RustWorkspace};

use crate::lock::{OvenRustInspectSourceAuthorityRequest, RustInspectWorkspaceRequest};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Real Rust Loafs, including a unit outside the consumer's declared roots, and an explicit compiler request.
struct Fixture {
    root: tempfile::TempDir,
    rustc: PathBuf,
    target: String,
    toolchain: String,
    lock: PathBuf,
    output: PathBuf,
    facets: [NativeLoafFacet; 2],
}

impl Fixture {
    /// Create isolated authored inputs without a registry, SDK inventory or fabricated native artifact.
    fn new() -> TestResult<Self> {
        let root = tempfile::tempdir()?;
        let lock = root.path().join("lock.json");
        fs::write(&lock, r#"{"schema":"incan.oven.loaf-resolution/2","units":[]}"#)?;
        for name in ["probe_leaf", "unselected_leaf"] {
            let project = root.path().join(name);
            fs::create_dir_all(project.join("src"))?;
            fs::write(
                project.join("loaf.toml"),
                format!(
                    "[project]\nname='{name}'\nversion='1.0.0'\n[rust]\nname='{name}'\ntype='lib'\nedition='2024'\n"
                ),
            )?;
            fs::write(project.join("src/lib.rs"), "pub fn answer() -> i64 { 42 }\n")?;
        }
        let rustc = resolve_active_rustc()?;
        let target = rustc_host_target(&rustc)?;
        let toolchain = rustc_identity(&rustc)?;
        let output = root.path().join("native");
        Ok(Self {
            root,
            rustc,
            target,
            toolchain,
            lock,
            output,
            facets: ["probe_leaf", "unselected_leaf"].map(|name| NativeLoafFacet {
                project: name.into(),
                features: Vec::new(),
                domain: "target".to_string(),
            }),
        })
    }

    /// Supply the complete real producer request, retaining both selected and unselected units.
    fn request(&self) -> NativeLoafPreparationRequest<'_> {
        NativeLoafPreparationRequest {
            lock: &self.lock,
            blobs: self.root.path(),
            output: &self.output,
            rustc: &self.rustc,
            index: self.root.path(),
            index_commit: "0000000000000000000000000000000000000000",
            target: &self.target,
            profile: "debug",
            facet_owner: self.root.path(),
            facets: &self.facets,
        }
    }

    /// Exact authored path dependency declared by the consumer; the unselected unit remains in the full request.
    fn dependencies(&self) -> Vec<DependencySpec> {
        vec![DependencySpec {
            crate_name: "probe_leaf".to_string(),
            version: Some("=1.0.0".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: self.root.path().join("probe_leaf"),
            },
            optional: false,
            package: Some("probe_leaf".to_string()),
        }]
    }
}

/// Exercise the command's real ordinary entry rather than loading a saved graph directly.
fn prepare(
    fixture: &Fixture,
    project: &Path,
    observation: oven_rustc::native_loaf::NativeLoafRequestObservation,
    query: &str,
    profile: &str,
    derives: &[String],
) -> TestResult<crate::lock::PreparedRustInspectWorkspace> {
    fs::create_dir_all(project)?;
    let dependencies = fixture.dependencies();
    let resolved = ResolvedDependencies {
        dependencies: dependencies.clone(),
        dev_dependencies: Vec::new(),
    };
    let requirements = ProjectRequirements::default();
    let inputs = BTreeMap::new();
    let queries = [query.to_string()];
    Ok(super::super::prepare_rust_inspect_workspace_with_ordinary_native(
        RustInspectWorkspaceRequest {
            project_root: project,
            project_name: "ordinary_inspection_consumer",
            cargo_package_name: "ordinary_inspection_consumer",
            rust_edition: Some("2024".to_string()),
            resolved: &resolved,
            project_requirements: &requirements,
            lock_payload: None,
            cargo_lock_projection_root: None,
            clear_cargo_lock: false,
            cargo_policy_flags: Vec::new(),
            cargo_target_dir: &project.join("inspection-target"),
            rust_inspect_query_paths: &queries,
            rust_derive_probe_paths: derives,
            prepare_when_empty: true,
            direct_oven_inspection: true,
            force_direct_prewarm: false,
            oven_source_authority: Some(OvenRustInspectSourceAuthorityRequest {
                project_version: "1.0.0",
                target: &fixture.target,
                toolchain: &fixture.toolchain,
                profile,
                features: &[],
                build_unit_inputs: &inputs,
                registry_dependencies: &dependencies,
            }),
            prepared_project_source_authorities: None,
            explicit_oven_bake: false,
        },
        observation,
        &fixture.rustc,
    )?
    .ok_or("ordinary inspection did not return its retained workspace")?)
}

/// Extract genuine metadata, reuse it, refuse serialized reloads and retain the full request across semantic handoffs.
#[test]
fn ordinary_native_source_inspection_retains_original_owners_and_cached_database() -> TestResult {
    let fixture = Fixture::new()?;
    let native = prepare_native_loafs(&fixture.request())?;
    assert_eq!(native.report().compiled.len(), 2);
    let observation = native.request_observation()?;
    let original = observation
        .graph()
        .units()
        .values()
        .next()
        .ok_or("original native owner missing")?;
    let retained = Arc::downgrade(original);
    let repeated = prepare_native_loafs(&fixture.request())?;
    assert!(repeated.report().compiled.is_empty());
    assert_eq!(repeated.report().reused.len(), 2);
    let prepared = prepare(
        &fixture,
        &fixture.root.path().join("consumer"),
        observation.clone(),
        "probe_leaf::answer",
        "debug",
        &[],
    )?;
    prepared.verify_ordinary_native()?;
    let cache = RustMetadataCache::new();
    let first = cache.get_or_extract_complete(prepared.manifest_dir(), "probe_leaf::answer", &|_| {})?;
    assert_eq!(first.canonical_path, "probe_leaf::answer");
    let incan_lang::interop::metadata::RustItemKind::Function(signature) = &first.kind else {
        return Err("ordinary source did not yield callable metadata".into());
    };
    assert_eq!(signature.return_type, "i64");
    assert!(signature.params.is_empty());
    let second = cache.get_or_extract_complete(prepared.manifest_dir(), "probe_leaf::answer", &|_| {})?;
    assert!(Arc::ptr_eq(&first, &second));
    let reuse = super::OrdinaryInspectionProject {
        observation: observation.clone(),
        inputs: observation.graph().inspection_inputs()?,
        rustc: fixture.rustc.clone(),
        intent: oven_store::OvenBuildIntent {
            target: fixture.target.clone(),
            toolchain: fixture.toolchain.clone(),
            profile: "debug".to_string(),
            features: Vec::new(),
        },
        manifest_dir: prepared.manifest_dir().to_path_buf(),
    };
    assert!(cache.prepare_retained_project(
        prepared.manifest_dir(),
        &fixture.root.path().join("reused-inspection-target"),
        Box::new(reuse),
        &|_| {},
    )?);
    assert!(!fixture.root.path().join("reused-inspection-target").exists());
    let refused = RustWorkspace::load_with_options(prepared.manifest_dir(), &|_| {}, false);
    assert!(
        matches!(refused, Err(rust_inspect::RustMetadataError::LoadWorkspace { message, .. }) if message.contains("original retained producer capability"))
    );

    for (name, query, profile, derives) in [
        ("undeclared", "unselected_leaf::answer", "debug", Vec::new()),
        ("wrong-profile", "probe_leaf::answer", "release", Vec::new()),
        (
            "derive",
            "probe_leaf::answer",
            "debug",
            vec!["probe_leaf::UnownedDerive".to_string()],
        ),
    ] {
        assert!(
            prepare(
                &fixture,
                &fixture.root.path().join(name),
                observation.clone(),
                query,
                profile,
                &derives
            )
            .is_err()
        );
    }
    // An unselected source edit must invalidate the retained complete request, even though the queried leaf is
    // unchanged.
    let unselected = fixture.root.path().join("unselected_leaf/src/lib.rs");
    let bytes = fs::read(&unselected)?;
    fs::write(&unselected, "pub fn answer() -> i64 { 43 }\n")?;
    assert!(prepared.verify_ordinary_native().is_err());
    fs::write(&unselected, bytes)?;
    prepared.verify_ordinary_native()?;
    let original_source = {
        let sources = observation.graph().inspection_inputs()?;
        sources
            .units()
            .values()
            .next()
            .ok_or("original inspection source missing")?
            .root_module()
            .to_path_buf()
    };
    let moved = original_source.with_extension("retained-original");
    fs::rename(&original_source, &moved)?;
    assert!(prepared.verify_ordinary_native().is_err());
    fs::rename(moved, original_source)?;
    prepared.verify_ordinary_native()?;
    let manifest_dir = prepared.manifest_dir().to_path_buf();
    drop(prepared);
    drop(repeated);
    drop(native);
    drop(observation);
    assert!(retained.upgrade().is_some());
    assert!(
        cache
            .get_or_extract_complete(&manifest_dir, "probe_leaf::answer", &|_| {})
            .is_ok()
    );
    cache.invalidate_manifest_dir(&manifest_dir)?;
    assert!(retained.upgrade().is_none());
    assert!(
        cache
            .get_or_extract_complete(&manifest_dir, "probe_leaf::answer", &|_| {})
            .is_err()
    );
    Ok(())
}

/// An invalid source graph used only to prove refusal before rust-analyzer or macro execution can start.
struct UnownedMacroGraph;

impl rust_inspect::RetainedInspectionProject for UnownedMacroGraph {
    /// Return a deliberately unowned nested macro path, never a successful native capability.
    fn verified_project(&self) -> Result<serde_json::Value, rust_inspect::RustMetadataError> {
        Ok(serde_json::json!({"crates": [{"nested": {"proc_macro_dylib_path": null}}]}))
    }
}

/// A retained source request does not grant executable macro authority, even for nested or null paths.
#[test]
fn ordinary_source_loader_refuses_unowned_macro_paths_before_loading() -> TestResult {
    let root = tempfile::tempdir()?;
    let target = root.path().join("never-created");
    let result = RustWorkspace::load_retained_oven_project(root.path(), &target, Box::new(UnownedMacroGraph), &|_| {});
    assert!(
        matches!(result, Err(rust_inspect::RustMetadataError::LoadWorkspace { message, .. }) if message.contains("does not grant native macro execution"))
    );
    assert!(!target.exists());
    let result =
        RustMetadataCache::new().prepare_retained_project(root.path(), &target, Box::new(UnownedMacroGraph), &|_| {});
    assert!(
        matches!(result, Err(rust_inspect::RustMetadataError::LoadWorkspace { message, .. }) if message.contains("projection markers"))
    );
    assert!(!target.exists());
    Ok(())
}
