//! Rust crate roots enter the existing test inventory, scheduler and reports through native libtest metadata.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use incan_driver::features::FeatureSelection;
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
    let source = manifest.rust_source_directory();
    let mut candidates = vec![manifest.rust_library_path(), source.join("main.rs")];
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

/// Current declaration-selected inputs for one harness, before any shared dependency compilation starts.
struct ResolvedRustRoot {
    source: PathBuf,
    declaration: PathBuf,
    declaration_digest: String,
    target_declaration: bool,
    manifest: ProjectManifest,
    features: Vec<String>,
    dependencies: Vec<oven_model::manifest::DependencySpec>,
    rustc: PathBuf,
    target: String,
}

impl ResolvedRustRoot {
    /// A declaration added or changed while another harness compiles invalidates this command's selected group.
    fn verify_declaration(&self) -> Result<(), Box<dyn std::error::Error>> {
        if self.source.with_extension("loaf.toml").is_file() != self.target_declaration
            || oven_store::store::digest_regular_file(&self.declaration)?.1 != self.declaration_digest
        {
            return Err("Rust test declaration changed during grouped preparation".into());
        }
        Ok(())
    }
}

/// Compile the selected roots through one existing producer, preserving each harness's direct imports.
/// Compatible requirements are united only after ordinary feature/environment/target selection. Conflicting
/// declarations retain their independent preparation; no selected root or additional feature is invented here.
pub(super) fn prepare_roots(
    sources: &[PathBuf],
    context: &OvenTestCommandContext,
    features: &FeatureSelection,
    owner: &mut incan_driver::build::executable::ExecutableBuildOwner,
) -> CliResult<Vec<(PathBuf, PreparedRustRoot)>> {
    let roots = sources
        .iter()
        .map(|source| {
            resolve_root(source, context, features, owner)
                .map_err(|e| CliError::failure(format!("Rust test {}: {e}", source.display())))
        })
        .collect::<CliResult<Vec<_>>>()?;
    let shared = shared_dependencies(&roots);
    let preparation_output = context.project_root.join("target/oven-tests/native-dependencies");
    let mut prepared = Vec::new();
    for root in &roots {
        for observed in &roots {
            observed
                .verify_declaration()
                .map_err(|e| CliError::failure(e.to_string()))?;
        }
        let result = prepare_inner(root, context, owner, shared.as_deref(), &preparation_output)
            .map_err(|e| CliError::failure(format!("Rust test {}: {e}", root.source.display())))?;
        prepared.push((root.source.clone(), result));
    }
    for root in &roots {
        root.verify_declaration()
            .map_err(|e| CliError::failure(e.to_string()))?;
    }
    Ok(prepared)
}

/// Unite identical authored aliases without expanding features or merging incompatible source/version requests.
fn shared_dependencies(roots: &[ResolvedRustRoot]) -> Option<Vec<oven_model::manifest::DependencySpec>> {
    let first = roots.first()?;
    if roots.len() < 2 {
        return None;
    }
    let mut dependencies = BTreeMap::new();
    for root in roots {
        if root.manifest.project_root() != first.manifest.project_root()
            || root.rustc != first.rustc
            || root.target != first.target
        {
            return None;
        }
        for dependency in &root.dependencies {
            if let Some(previous) = dependencies.get(&dependency.crate_name) {
                if previous != dependency {
                    return None;
                }
            } else {
                dependencies.insert(dependency.crate_name.clone(), dependency.clone());
            }
        }
    }
    Some(dependencies.into_values().collect())
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

/// A procedural macro's own library harness needs the selected toolchain's dynamic `proc_macro` support.
/// Integration tests remain ordinary binaries even when their owning package declares a procedural macro.
/// Resolve the same authored source root as discovery; a sibling test declaration cannot promote another root.
fn proc_macro_test_root(manifest: &ProjectManifest, source: &Path) -> Result<bool, std::io::Error> {
    if !manifest
        .rust_facet
        .as_ref()
        .is_some_and(|facet| facet.kind.as_deref() == Some("proc-macro"))
    {
        return Ok(false);
    }
    let library = manifest.rust_library_path();
    if !library.is_file() {
        return Ok(false);
    }
    Ok(library.canonicalize()? == source.canonicalize()?)
}

/// Independent binary and integration-test roots link their own authored library as an ordinary local dependency.
/// Library unit tests compile that source directly. A sibling declaration is already a complete target contract and
/// must not inherit an implicit self dependency from the package containing it.
fn test_dependencies(
    manifest: &ProjectManifest,
    features: &incan_provider::ResolvedPackageFeatures,
    source: &Path,
    has_target_declaration: bool,
) -> Result<Vec<oven_model::manifest::DependencySpec>, Box<dyn std::error::Error>> {
    let mut dependencies = crate::commands::oven::native_compiler_tests::native_test_dependencies(manifest, features)?;
    if has_target_declaration {
        return Ok(dependencies);
    }
    let root = manifest.project_root();
    let library = manifest.rust_library_path();
    if library.is_file() && library.canonicalize()? != source.canonicalize()? {
        let name = manifest
            .rust_facet
            .as_ref()
            .and_then(|facet| facet.name.as_deref())
            .or_else(|| manifest.project.as_ref().and_then(|project| project.name.as_deref()))
            .ok_or("Rust test package has no library identity")?
            .replace('-', "_");
        if dependencies.iter().any(|dependency| dependency.crate_name == name) {
            return Err(format!("Rust test library `{name}` has a competing dependency declaration").into());
        }
        dependencies.push(oven_model::manifest::DependencySpec {
            crate_name: name,
            version: None,
            features: features.active_features.iter().cloned().collect(),
            default_features: false,
            source: oven_model::manifest::DependencySource::Path {
                path: root.to_path_buf(),
            },
            optional: false,
            package: None,
        });
    }
    Ok(dependencies)
}

/// Resolve one root once using the canonical test manifest, feature and target-decision machinery.
fn resolve_root(
    source: &Path,
    context: &OvenTestCommandContext,
    features: &FeatureSelection,
    owner: &mut incan_driver::build::executable::ExecutableBuildOwner,
) -> Result<ResolvedRustRoot, Box<dyn std::error::Error>> {
    let target_declaration = source.with_extension("loaf.toml").is_file();
    let declaration = if target_declaration {
        source.with_extension("loaf.toml")
    } else {
        context.project_root.join("loaf.toml")
    };
    let declaration_digest = oven_store::store::digest_regular_file(&declaration)?.1;
    let owned_manifest = if target_declaration {
        Some(ProjectManifest::load(&declaration)?)
    } else {
        None
    };
    let manifest = owned_manifest
        .as_ref()
        .or(context.session.manifest.as_ref())
        .ok_or("Rust tests require loaf.toml")?;
    let (effective_manifest, selected_features) = resolve_test_manifest(manifest, features)?;
    let rustc = resolve_active_rustc()?;
    let target = rustc_host_target(&rustc)?;
    let selected_manifest = crate::commands::oven::native_compiler_tests::selected_test_manifest(
        &effective_manifest,
        owner,
        &rustc,
        &target,
    )?;
    let manifest = &selected_manifest;
    if manifest
        .rust_facet
        .as_ref()
        .is_some_and(|facet| facet.build_script == Some(true))
    {
        return Err("Rust test roots with build scripts require declared build facts in their native plan".into());
    }
    let dependencies = test_dependencies(manifest, &selected_features, source, owned_manifest.is_some())?;
    let root = ResolvedRustRoot {
        source: source.to_path_buf(),
        declaration,
        declaration_digest,
        target_declaration,
        manifest: selected_manifest,
        features: selected_features.active_features.into_iter().collect(),
        dependencies,
        rustc,
        target,
    };
    root.verify_declaration()?;
    Ok(root)
}

/// Preserve exact compilation inputs and leases while mapping real libtest metadata into common test records.
fn prepare_inner(
    root: &ResolvedRustRoot,
    context: &OvenTestCommandContext,
    owner: &mut incan_driver::build::executable::ExecutableBuildOwner,
    shared_dependencies: Option<&[oven_model::manifest::DependencySpec]>,
    preparation_output: &Path,
) -> Result<PreparedRustRoot, Box<dyn std::error::Error>> {
    let _stage = trace_test_stage("rust_test_preparation");
    let source = root.source.as_path();
    let declaration = &root.declaration;
    let manifest = &root.manifest;
    let features = &root.features;
    let dependencies = &root.dependencies;
    let rustc = root.rustc.as_path();
    let target = &root.target;
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
        target.as_str(),
        rustc_identity(rustc)?,
        "debug",
        features.clone(),
    )
    .with_generated_source("test-root", source)
    .with_generated_source_tree("test-modules", source.parent().ok_or("Rust root has no parent")?)
    .with_generated_source("test-declaration", declaration)
    .with_build_unit_input("test-environment", serde_json::to_string(&compile_environment)?);
    let source_receipt = receipt_generated_project(&request)?;
    let native = incan_driver::build::native_loaf_plan::ConfiguredNativeLoafInputs::discover()?
        .ok_or("Rust tests require the toolchain's ordinary native Loaf producer coordinates")?;
    let root_key = hex::encode(Sha256::digest(source.to_string_lossy().as_bytes()));
    let directory = context.project_root.join("target/oven-tests").join(root_key);
    std::fs::create_dir_all(&directory)?;
    let native_hints = directory.join("native-hints");
    let native_request = oven_rustc::native_loaf::NativeLoafConsumerRequest {
        graph: &native.graph,
        index: &native.index,
        blobs: &native.blobs,
        output: &native_hints,
        rustc,
        target: &source_receipt.intent.target,
        profile: &source_receipt.intent.profile,
        dependencies,
        declaration_owner: manifest.project_root(),
        domain: "target",
    };
    let (receipt, selection, _, _) = if let Some(dependencies) = shared_dependencies {
        owner.native_loaf_plan_in_group(
            &context.store,
            &source_receipt,
            &native_request,
            dependencies,
            preparation_output,
        )?
    } else {
        owner.native_loaf_plan(&context.store, &source_receipt, &native_request)?
    };
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
            rustc,
            source,
            output: &executable,
            crate_name: &name,
            edition,
            source_evidence_key: "test-root",
            features,
            prefer_dynamic: proc_macro_test_root(manifest, source)?,
        },
        &context.store,
    )?;
    // Native test helpers consume the same leased closure as their root. Both existing child consumers receive
    // that checked plan: generated-code checks and vocabulary extraction must not discover another dependency graph.
    let mut environment = environment;
    // Insta otherwise starts Cargo to discover a workspace. Its runtime override belongs to the child
    // environment, not the compilation receipt; preserve an explicitly configured snapshot workspace.
    environment.insert(
        "INSTA_WORKSPACE_ROOT".into(),
        std::env::var("INSTA_WORKSPACE_ROOT").unwrap_or_else(|_| context.project_root.to_string_lossy().into_owned()),
    );
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
        rustc,
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

/// Execute one exact Rust-root selection through the same supervisor and private result channel as isolation.
pub(super) fn execute(
    tests: &[TestInfo],
    context: &OvenTestCommandContext,
    timeout: Option<Duration>,
    no_capture: bool,
    output: &TestOutputSink,
    shared_threads: Option<usize>,
) -> Vec<(TestInfo, TestResult)> {
    let Some(first) = tests.first() else {
        return Vec::new();
    };
    let started = Instant::now();
    let report = (|| {
        let root = context
            .rust_roots
            .get(&first.file_path)
            .ok_or("Rust root was not in the prepared inventory")?;
        if tests.iter().any(|test| test.file_path != first.file_path) {
            return Err("shared Rust execution crosses prepared roots".to_string());
        }
        run_native_tests_exact_for_request(
            &OvenNativeTestRequest {
                executable: root.executable.clone(),
                exact_names: tests.iter().map(|test| test.function_name.clone()).collect(),
                environment: root.environment.clone(),
                timeout,
            },
            &root.working_directory,
            no_capture,
            Some(&root.collection),
            shared_threads,
        )
        .map_err(|error| error.to_string())
    })();
    if no_capture && let Ok(report) = &report {
        output.stdout(report.output.clone());
    }
    tests
        .iter()
        .map(|test| {
            let result = match &report {
                Err(error) => TestResult::Failed(started.elapsed(), error.clone()),
                Ok(report) => {
                    let valid = report
                        .case_counts
                        .as_ref()
                        .is_some_and(|counts| !report.timed_out && (report.process_success || counts.failed > 0));
                    let case = report.case_results.iter().find(|case| case.name == test.function_name);
                    match case.filter(|_| valid) {
                        Some(case) if case.outcome == "ok" => {
                            TestResult::Passed(Duration::from_millis(case.elapsed_ms.unwrap_or(0)))
                        }
                        Some(case) if case.outcome == "ignored" => TestResult::Skipped("Rust #[ignore]".into()),
                        _ => TestResult::Failed(started.elapsed(), report.output.clone()),
                    }
                }
            };
            (test.clone(), result)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::discover_roots;

    /// Grouping preserves authored feature/source choices and notices declaration changes during preparation.
    #[test]
    fn native_harness_group_keeps_conflicts_separate_and_refuses_changed_declarations()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let root = project.path();
        let declaration = root.join("loaf.toml");
        std::fs::create_dir_all(root.join("leaf/src"))?;
        std::fs::write(root.join("leaf/src/lib.rs"), "pub const VALUE: u8 = 1;")?;
        std::fs::write(
            root.join("leaf/loaf.toml"),
            "[project]\nname='leaf'\n[rust]\ntype='lib'\n",
        )?;
        std::fs::write(
            &declaration,
            "[project]\nname='grouped'\n[dependencies]\nleaf={loaf='leaf',path='leaf',features=['base']}\n",
        )?;
        let manifest = oven_model::manifest::ProjectManifest::load(&declaration)?;
        let dependencies = manifest.rust_dependency_values();
        let inputs = |name: &str| -> Result<super::ResolvedRustRoot, Box<dyn std::error::Error>> {
            Ok(super::ResolvedRustRoot {
                source: root.join(name),
                declaration: declaration.clone(),
                declaration_digest: oven_store::store::digest_regular_file(&declaration)?.1,
                target_declaration: false,
                manifest: manifest.clone(),
                features: Vec::new(),
                dependencies: dependencies.clone(),
                rustc: "selected-rustc".into(),
                target: "selected-target".into(),
            })
        };
        let mut roots = vec![inputs("first.rs")?, inputs("second.rs")?];
        assert_eq!(super::shared_dependencies(&roots), Some(dependencies.clone()));
        roots[1]
            .dependencies
            .first_mut()
            .ok_or("native leaf dependency missing")?
            .features
            .push("different".into());
        assert!(
            super::shared_dependencies(&roots).is_none(),
            "grouping must not expand an authored feature set"
        );
        roots[1].dependencies = dependencies;
        roots[1]
            .dependencies
            .first_mut()
            .ok_or("native leaf dependency missing")?
            .source = oven_model::manifest::DependencySource::Path {
            path: root.join("different-leaf"),
        };
        assert!(
            super::shared_dependencies(&roots).is_none(),
            "same alias must not hide a different source"
        );
        roots[0].verify_declaration()?;
        std::fs::write(root.join("first.loaf.toml"), "[project]\nname='target-contract'\n")?;
        assert!(
            roots[0].verify_declaration().is_err(),
            "new target declaration was ignored"
        );
        std::fs::write(&declaration, "[project]\nname='changed'\n")?;
        assert!(
            roots[1].verify_declaration().is_err(),
            "changed package declaration was ignored"
        );
        Ok(())
    }

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
        let selection = incan_driver::features::FeatureSelection {
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
        let unknown = incan_driver::features::FeatureSelection {
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
        let selection = incan_driver::features::FeatureSelection {
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

    /// Discovery, macro classification and self-library linkage share the admitted custom library path.
    #[test]
    fn authored_library_root_controls_discovery_and_linkage() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let root = project.path();
        std::fs::create_dir_all(root.join("rust/src"))?;
        std::fs::create_dir_all(root.join("tests"))?;
        let declaration = "[project]\nname='custom'\n[rust]\ntype='proc-macro'\npath='rust/src/user_doc.rs'\n[rust.source]\nroot='rust'\n";
        std::fs::write(root.join("loaf.toml"), declaration)?;
        for file in ["rust/src/user_doc.rs", "rust/src/lib.rs", "tests/check.rs"] {
            std::fs::write(root.join(file), "")?;
        }
        assert_eq!(
            super::discover_roots(root)?,
            vec![
                root.join("rust/src/user_doc.rs").canonicalize()?,
                root.join("tests/check.rs").canonicalize()?
            ]
        );
        let manifest = oven_model::manifest::ProjectManifest::load(&root.join("loaf.toml"))?;
        assert!(super::proc_macro_test_root(
            &manifest,
            &root.join("rust/src/user_doc.rs")
        )?);
        assert!(!super::proc_macro_test_root(&manifest, &root.join("tests/check.rs"))?);
        let (_, features) = super::resolve_test_manifest(&manifest, &Default::default())?;
        assert!(super::test_dependencies(&manifest, &features, &root.join("rust/src/user_doc.rs"), false)?.is_empty());
        assert_eq!(
            super::test_dependencies(&manifest, &features, &root.join("tests/check.rs"), false)?.len(),
            1
        );
        for invalid in [
            "../outside.rs",
            "/tmp/outside.rs",
            "rust/src/../outside.rs",
            "src/wrong.rs",
            "rust/src/not-rust.txt",
        ] {
            assert!(
                oven_model::manifest::ProjectManifest::from_str(
                    &declaration.replace("rust/src/user_doc.rs", invalid),
                    &root.join("loaf.toml")
                )
                .is_err()
            );
        }
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

    /// Integration tests receive the authored library and selected features; unit and explicitly declared roots do not.
    #[test]
    fn independent_roots_link_the_selected_package_library() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let root = project.path();
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::create_dir_all(root.join("tests"))?;
        let library = root.join("src/lib.rs");
        let integration = root.join("tests/check.rs");
        std::fs::write(&library, "")?;
        std::fs::write(&integration, "")?;
        let manifest = oven_model::manifest::ProjectManifest::from_str(
            "[project]\nname = \"own-library\"\n[project.features]\ntest_support = []\n",
            &root.join("loaf.toml"),
        )?;
        let selected = incan_driver::features::FeatureSelection {
            requested: std::collections::BTreeSet::from(["test_support".into()]),
            ..Default::default()
        };
        let (effective, features) = super::resolve_test_manifest(&manifest, &selected)?;
        assert!(super::test_dependencies(&effective, &features, &library, false)?.is_empty());
        assert!(super::test_dependencies(&effective, &features, &integration, true)?.is_empty());
        let dependencies = super::test_dependencies(&effective, &features, &integration, false)?;
        assert_eq!(dependencies.len(), 1);
        assert_eq!(dependencies[0].crate_name, "own_library");
        assert!(dependencies[0].features.contains(&"test_support".to_string()));
        assert!(!dependencies[0].default_features);
        assert_eq!(
            dependencies[0].source,
            oven_model::manifest::DependencySource::Path {
                path: root.to_path_buf()
            }
        );
        Ok(())
    }

    /// Only the authored macro library receives sysroot macro support, including non-default source layouts.
    #[test]
    fn macro_harness_mode_follows_authored_library_root() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let root = project.path();
        std::fs::create_dir_all(root.join("rust/src"))?;
        std::fs::create_dir_all(root.join("tests"))?;
        let library = root.join("rust/src/lib.rs");
        let integration = root.join("tests/check.rs");
        std::fs::write(&library, "")?;
        std::fs::write(&integration, "")?;
        let declaration =
            "[project]\nname = \"macro-root\"\n[rust]\ntype = \"proc-macro\"\n[rust.source]\nroot = \"rust\"\n";
        let manifest = oven_model::manifest::ProjectManifest::from_str(declaration, &root.join("loaf.toml"))?;
        assert!(super::proc_macro_test_root(&manifest, &library.canonicalize()?)?);
        assert!(!super::proc_macro_test_root(&manifest, &integration)?);
        let ordinary = oven_model::manifest::ProjectManifest::from_str(
            &declaration.replace("proc-macro", "lib"),
            &root.join("loaf.toml"),
        )?;
        assert!(!super::proc_macro_test_root(&ordinary, &library)?);
        let sibling =
            oven_model::manifest::ProjectManifest::from_str(declaration, &root.join("tests/check.loaf.toml"))?;
        assert!(!super::proc_macro_test_root(&sibling, &integration)?);
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
