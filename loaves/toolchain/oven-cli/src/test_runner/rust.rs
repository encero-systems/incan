//! Rust crate roots enter the existing test inventory, scheduler and reports through native libtest metadata.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use incan_provider::FeatureSelection;
use oven_model::manifest::ProjectManifest;
use oven_rustc::native_test::{OvenNativeTestCollection, OvenNativeTestRequest, run_native_tests_exact_for_request};
use oven_rustc::plan::OvenDirectRustcPlanSelection;
use oven_rustc::rustc::{
    OvenTrustedDirectRustcTargetRequest, bake_trusted_direct_rustc_test_in_store, resolve_active_rustc,
    rustc_host_target, rustc_identity,
};
use oven_store::{OvenGeneratedProjectRequest, receipt_generated_project};
use sha2::{Digest, Sha256};

use super::execution::OvenTestCommandContext;
use super::output::TestOutputSink;
use super::{TestInfo, TestMarker, TestResult, trace_test_stage};
use crate::{CliError, CliResult};

/// The root's compiled harness and original dependency leases remain alive until all scheduled cases finish.
pub(super) struct PreparedRustRoot {
    /// Actual libtest inventory mapped to the common selection model.
    pub(super) tests: Vec<TestInfo>,
    executable: PathBuf,
    collection: Arc<OvenNativeTestCollection>,
    environment: BTreeMap<String, String>,
    working_directory: PathBuf,
    _selection: Arc<OvenDirectRustcPlanSelection>,
}

/// Locate the nearest authored project for either a directory or source-file selection.
/// Use the manifest's existing discovery rules rather than a source-file-only parent heuristic.
pub(super) fn project_root(path: &Path) -> CliResult<Option<PathBuf>> {
    let path = path
        .canonicalize()
        .map_err(|error| CliError::failure(error.to_string()))?;
    let start = if path.is_dir() {
        path.as_path()
    } else {
        path.parent()
            .ok_or_else(|| CliError::failure("test path has no parent"))?
    };
    oven_model::manifest::find_manifest(start)
        .map_err(|error| CliError::failure(error.to_string()))?
        .map(|manifest| {
            manifest
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| CliError::failure("test manifest has no parent"))
        })
        .transpose()
}

/// Discover authored crate roots, never treating nested Rust modules as independent crates.
/// The selected path narrows the package's conventional roots and declared binary roles.
pub(super) fn discover_roots(path: &Path) -> CliResult<Vec<PathBuf>> {
    let Some(root) = project_root(path)? else {
        return Ok(Vec::new());
    };
    let manifest_path = root.join("loaf.toml");
    let manifest = ProjectManifest::load(&manifest_path).map_err(|e| CliError::failure(e.to_string()))?;
    let source = manifest
        .rust_source
        .as_ref()
        .map_or_else(|| root.join("src"), |source| root.join(&source.root).join("src"));
    let mut candidates = vec![source.join("lib.rs"), source.join("main.rs")];
    candidates.extend(manifest.rust_bins.iter().map(|role| root.join(&role.path)));
    let tests = root.join("tests");
    if tests.is_dir() {
        for entry in std::fs::read_dir(&tests).map_err(|e| CliError::failure(e.to_string()))? {
            let entry = entry.map_err(|e| CliError::failure(e.to_string()))?;
            let candidate = entry.path();
            if candidate.extension().is_some_and(|extension| extension == "rs") {
                candidates.push(candidate);
            }
        }
    }
    let selected = path.canonicalize().map_err(|e| CliError::failure(e.to_string()))?;
    let root = root.canonicalize().map_err(|e| CliError::failure(e.to_string()))?;
    let mut roots = BTreeSet::new();
    for candidate in candidates.into_iter().filter(|candidate| candidate.is_file()) {
        let candidate = candidate.canonicalize().map_err(|e| CliError::failure(e.to_string()))?;
        if !candidate.starts_with(&root) {
            return Err(CliError::failure("Rust test root escapes its project"));
        }
        if candidate == selected || (selected.is_dir() && candidate.starts_with(&selected)) {
            roots.insert(candidate);
        }
    }
    Ok(roots.into_iter().collect())
}

/// Compile one authored Rust root through the existing receipt, native admission and direct-rustc plan selection.
/// A sibling declaration may narrow integration-test dependencies; it never supplies ambient Cargo authority.
pub(super) fn prepare(
    source: &Path,
    context: &OvenTestCommandContext,
    features: &FeatureSelection,
    owner: &mut incan_driver::build::executable::ExecutableBuildOwner,
) -> CliResult<PreparedRustRoot> {
    prepare_inner(source, context, features, owner)
        .map_err(|e| CliError::failure(format!("Rust test {}: {e}", source.display())))
}

/// Resolve public features against the authored dependency contract before applying test-only requirements.
/// An environment may make an optional dependency mandatory; that must not invalidate the original `dep:` edge.
/// Both selections still use the canonical feature graph and environment resolver, without mutating the source.
fn resolve_test_manifest(
    manifest: &ProjectManifest,
    features: &FeatureSelection,
) -> Result<(ProjectManifest, incan_provider::ResolvedPackageFeatures), Box<dyn std::error::Error>> {
    let selected = incan_provider::PackageFeatureGraph::from_manifest(manifest)?.resolve(features)?;
    let environment = oven_model::project_lifecycle::env::EnvConfigSet::from_manifest(manifest).resolve_env("test")?;
    Ok((
        manifest.with_environment_dependencies(&environment.dependencies)?,
        selected,
    ))
}

/// Preserve exact compilation inputs and leases while mapping real libtest metadata into common test records.
fn prepare_inner(
    source: &Path,
    context: &OvenTestCommandContext,
    features: &FeatureSelection,
    owner: &mut incan_driver::build::executable::ExecutableBuildOwner,
) -> Result<PreparedRustRoot, Box<dyn std::error::Error>> {
    let _stage = trace_test_stage("rust_test_preparation");
    let declaration = source.with_extension("loaf.toml");
    let owned_manifest = if declaration.is_file() {
        Some(ProjectManifest::load(&declaration)?)
    } else {
        None
    };
    let manifest = owned_manifest
        .as_ref()
        .or(context.session.manifest.as_ref())
        .ok_or("Rust tests require loaf.toml")?;
    let (effective_manifest, selected_features) = resolve_test_manifest(manifest, features)?;
    let manifest = &effective_manifest;
    if manifest
        .rust_facet
        .as_ref()
        .is_some_and(|facet| facet.build_script == Some(true))
    {
        return Err("Rust test roots with build scripts require declared build facts in their native plan".into());
    }
    let features = selected_features.active_features.iter().cloned().collect::<Vec<_>>();
    let dependencies =
        crate::commands::oven::native_compiler_tests::native_test_dependencies(manifest, &selected_features)?;
    let rustc = resolve_active_rustc()?;
    let project = manifest
        .project
        .as_ref()
        .ok_or("Rust test declaration has no project identity")?;
    let name = project
        .name
        .as_deref()
        .ok_or("Rust test declaration has no project name")?
        .replace('-', "_");
    let compiler = incan_driver::build::native_runtime_inputs::bound_compiler_executable()?;
    let environment = BTreeMap::from([
        (
            "CARGO_MANIFEST_DIR".into(),
            context.project_root.to_string_lossy().into_owned(),
        ),
        ("CARGO_BIN_EXE_incan".into(), compiler.to_string_lossy().into_owned()),
    ]);
    // Compile-time package metadata comes from the same authored version bound into the receipt.
    // Keep it out of the runtime environment so child tests cannot inherit another package identity.
    let version = project.version.as_deref().unwrap_or("0.1.0");
    let mut compile_environment = environment.clone();
    compile_environment.insert("CARGO_PKG_VERSION".into(), version.to_string());
    let request = OvenGeneratedProjectRequest::new(
        &context.project_root,
        &name,
        version,
        rustc_host_target(&rustc)?,
        rustc_identity(&rustc)?,
        "debug",
        features.clone(),
    )
    .with_generated_source("test-root", source)
    .with_generated_source_tree("test-modules", source.parent().ok_or("Rust root has no parent")?)
    .with_generated_source(
        "test-declaration",
        if declaration.is_file() {
            declaration
        } else {
            context.project_root.join("loaf.toml")
        },
    )
    .with_build_unit_input("test-environment", serde_json::to_string(&compile_environment)?);
    let source_receipt = receipt_generated_project(&request)?;
    let native = incan_driver::build::native_loaf_plan::ConfiguredNativeLoafInputs::discover()?
        .ok_or("Rust tests require the toolchain's ordinary native Loaf producer coordinates")?;
    let root_key = hex::encode(Sha256::digest(source.to_string_lossy().as_bytes()));
    let directory = context.project_root.join("target/oven-tests").join(root_key);
    std::fs::create_dir_all(&directory)?;
    let (receipt, selection, _, _) = owner.native_loaf_plan(
        &context.store,
        &source_receipt,
        &oven_rustc::native_loaf::NativeLoafConsumerRequest {
            graph: &native.graph,
            index: &native.index,
            blobs: &native.blobs,
            output: &directory.join("native-hints"),
            rustc: &rustc,
            target: &source_receipt.intent.target,
            profile: &source_receipt.intent.profile,
            dependencies: &dependencies,
            declaration_owner: manifest.project_root(),
            domain: "target",
        },
    )?;
    let mut plan = selection.source_artifact_plan("test-root")?;
    plan.compile_environment.extend(compile_environment.clone());
    let mut artifacts = selection.artifacts().clone();
    artifacts.compile_environment.extend(compile_environment);
    let executable = directory.join("native-libtest");
    let edition = manifest
        .rust_facet
        .as_ref()
        .and_then(|facet| facet.edition.as_deref())
        .unwrap_or("2024");
    bake_trusted_direct_rustc_test_in_store(
        &OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &artifacts,
            artifact_root: selection.output_guard_root(),
            artifact_plan: Some(&plan),
            rustc: &rustc,
            source,
            output: &executable,
            crate_name: &name,
            edition,
            source_evidence_key: "test-root",
            features: &features,
            prefer_dynamic: false,
        },
        &context.store,
    )?;
    // Native test helpers consume the same leased closure as their root. Both existing child consumers receive
    // that checked plan: generated-code checks and vocabulary extraction must not discover another dependency graph.
    let mut environment = environment;
    environment.insert("RUSTC".to_string(), rustc.display().to_string());
    environment.insert(
        oven_model::compiler_suite_env::OVEN_COMPILER_SUITE_RUSTC_ENV.to_string(),
        rustc.display().to_string(),
    );
    crate::commands::oven::append_compiler_suite_direct_rustc_environment(
        &mut environment,
        &[
            oven_store::compiler_suite_env::OVEN_COMPILER_SUITE_CAPABILITY_ENV,
            oven_model::compiler_suite_env::OVEN_COMPILER_SUITE_VOCAB_CAPABILITY_ENV,
        ],
        &rustc,
        &plan,
    )?;
    let collection = owner.native_test_collection(
        &executable,
        &environment,
        &context.project_root,
        Some(Duration::from_secs(30)),
    )?;
    let ignored = collection.ignored().names.iter().collect::<BTreeSet<_>>();
    let tests = collection
        .inventory()
        .names
        .iter()
        .cloned()
        .map(|name| {
            let markers = if ignored.contains(&name) {
                vec![TestMarker::Skip("Rust #[ignore]".into())]
            } else {
                Vec::new()
            };
            TestInfo {
                file_path: source.to_path_buf(),
                function_name: name,
                is_async: false,
                markers,
                required_fixtures: Vec::new(),
                parameter_names: Vec::new(),
                timeout: None,
                parametrize_call: None,
            }
        })
        .collect();
    Ok(PreparedRustRoot {
        tests,
        executable,
        collection,
        environment,
        working_directory: context.project_root.clone(),
        _selection: selection,
    })
}

/// Execute only selected Rust cases, freshly, under the common scheduler's timeout and output policy.
pub(super) fn execute(
    tests: &[TestInfo],
    context: &OvenTestCommandContext,
    timeout: Option<Duration>,
    no_capture: bool,
    output: &TestOutputSink,
) -> Vec<(TestInfo, TestResult)> {
    tests
        .iter()
        .map(|test| {
            let started = Instant::now();
            let result = match context.rust_roots.get(&test.file_path) {
                None => TestResult::Failed(started.elapsed(), "Rust root was not in the prepared inventory".into()),
                Some(root) => match run_native_tests_exact_for_request(
                    &OvenNativeTestRequest {
                        executable: root.executable.clone(),
                        exact_names: vec![test.function_name.clone()],
                        environment: root.environment.clone(),
                        timeout,
                    },
                    &root.working_directory,
                    no_capture,
                    Some(&root.collection),
                ) {
                    Ok(report) if report.success => {
                        if no_capture {
                            output.stdout(report.output);
                        }
                        TestResult::Passed(started.elapsed())
                    }
                    Ok(report) => TestResult::Failed(started.elapsed(), report.output),
                    Err(error) => TestResult::Failed(started.elapsed(), error.to_string()),
                },
            };
            (test.clone(), result)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::discover_roots;

    /// A test-only requirement cannot turn a valid authored optional-feature edge into a feature error.
    #[test]
    fn test_overlay_preserves_authored_optional_feature_contract() -> Result<(), Box<dyn std::error::Error>> {
        let manifest = oven_model::manifest::ProjectManifest::from_str(
            r#"
[project]
name = "feature-overlay"
[project.features]
helpers = ["dep:leaf", "leaf/extra"]
[dependencies]
leaf = { loaf = "crates-io/leaf", version = "1", optional = true }
[envs.test.dependencies]
leaf = { optional = false }
"#,
            std::path::Path::new("/fixture/loaf.toml"),
        )?;
        let selection = incan_provider::FeatureSelection {
            requested: std::collections::BTreeSet::from(["helpers".into()]),
            ..Default::default()
        };
        let (effective, features) = super::resolve_test_manifest(&manifest, &selection)?;
        assert!(features.active_features.contains("helpers"));
        assert!(features.active_optional_dependencies.contains("leaf"));
        assert!(
            features
                .dependency_features
                .get("leaf")
                .is_some_and(|items| items.contains("extra"))
        );
        assert!(manifest.loaf_dependencies["leaf"][0].spec.optional);
        assert!(!effective.loaf_dependencies["leaf"][0].spec.optional);
        let (_, default_features) = super::resolve_test_manifest(&manifest, &Default::default())?;
        assert!(!default_features.active_features.contains("helpers"));
        let unknown = incan_provider::FeatureSelection {
            requested: std::collections::BTreeSet::from(["unknown".into()]),
            ..Default::default()
        };
        assert!(super::resolve_test_manifest(&manifest, &unknown).is_err());
        Ok(())
    }

    /// Mixed-loaf Rust facets retain optional activation and dependency feature selection in the shared native
    /// projection.
    #[test]
    fn mixed_loaf_native_dependencies_preserve_selected_features() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let root = project.path();
        std::fs::create_dir_all(root.join("mixed/rust/src"))?;
        std::fs::write(root.join("mixed/rust/src/lib.rs"), "pub fn value() -> u32 { 42 }")?;
        std::fs::write(
            root.join("mixed/loaf.toml"),
            "[project]\nname = \"mixed\"\n[rust.source]\nroot = \"rust\"\n",
        )?;
        let manifest = oven_model::manifest::ProjectManifest::from_str(
            r#"
[project]
name = "mixed-consumer"
[project.features]
helpers = ["dep:mixed", "mixed/extra"]
[dependencies]
mixed = { loaf = "mixed", path = "mixed", optional = true, default-features = false, features = ["base"] }
"#,
            &root.join("loaf.toml"),
        )?;
        assert!(manifest.library_dependencies().contains_key("mixed"));
        let (effective, features) = super::resolve_test_manifest(&manifest, &Default::default())?;
        let inactive = crate::commands::oven::native_compiler_tests::native_test_dependencies(&effective, &features)?;
        assert!(inactive.is_empty());
        let selection = incan_provider::FeatureSelection {
            requested: std::collections::BTreeSet::from(["helpers".into()]),
            ..Default::default()
        };
        let (effective, features) = super::resolve_test_manifest(&manifest, &selection)?;
        let dependencies =
            crate::commands::oven::native_compiler_tests::native_test_dependencies(&effective, &features)?;
        assert_eq!(dependencies.len(), 1);
        let dependency = &dependencies[0];
        assert_eq!(dependency.crate_name, "mixed");
        assert_eq!(dependency.features, ["base", "extra"]);
        assert!(!dependency.default_features);
        assert!(dependency.optional);
        assert_eq!(
            dependency.source,
            oven_model::manifest::DependencySource::Path {
                path: root.join("mixed")
            }
        );
        Ok(())
    }

    /// Nested Rust modules are compiled by their authored crate root, never collected as sibling crates.
    #[test]
    fn discovery_honors_mixed_source_roots_and_path_selection() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let root = project.path();
        std::fs::create_dir_all(root.join("rust/src"))?;
        std::fs::create_dir_all(root.join("tests"))?;
        std::fs::write(
            root.join("loaf.toml"),
            "[project]\nname = \"mixed\"\n[rust.source]\nroot = \"rust\"\n",
        )?;
        for source in ["rust/src/lib.rs", "rust/src/helper.rs", "tests/check.rs"] {
            std::fs::write(root.join(source), "")?;
        }
        let roots = discover_roots(root)?;
        assert_eq!(
            roots,
            vec![
                root.join("rust/src/lib.rs").canonicalize()?,
                root.join("tests/check.rs").canonicalize()?
            ]
        );
        assert_eq!(
            discover_roots(&root.join("tests"))?,
            vec![root.join("tests/check.rs").canonicalize()?]
        );
        assert!(discover_roots(&root.join("rust/src/helper.rs"))?.is_empty());
        Ok(())
    }

    /// A source symlink cannot turn another package into the selected package's test authority.
    #[cfg(unix)]
    #[test]
    fn discovery_rejects_roots_outside_the_project() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let foreign = tempfile::tempdir()?;
        std::fs::create_dir_all(project.path().join("src"))?;
        std::fs::write(project.path().join("loaf.toml"), "[project]\nname = \"local\"\n")?;
        std::fs::write(foreign.path().join("lib.rs"), "")?;
        std::os::unix::fs::symlink(foreign.path().join("lib.rs"), project.path().join("src/lib.rs"))?;
        assert!(discover_roots(project.path()).is_err());
        Ok(())
    }
}
