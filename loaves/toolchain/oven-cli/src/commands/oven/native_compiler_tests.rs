//! Receipt-bound compiler test roots using authored dependencies and ordinary native Loafs.
//!
//! Native-only preparation supplies immutable dependencies; each sibling declaration supplies its direct aliases,
//! and the receipt binds the
//! complete test module tree and the explicit compilation environment before the existing direct-Rustc runner runs.
//! Local cfg features are resolved from the test declaration and bound before stored output selection.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::{CliError, CliResult, ExitCode};
use oven_model::manifest::ProjectManifest;
use oven_rustc::native_loaf::NativeLoafGraph;
use oven_rustc::rustc::{OvenTrustedDirectRustcTargetRequest, bake_trusted_direct_rustc_test_in_store};
use oven_store::store::{OvenStore, OvenStoreLimits};

mod preparation;

/// Execute one declared test root without converting ambient Cargo metadata or silently falling back to Cargo.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    compiler_root: PathBuf,
    target: PathBuf,
    declaration: Option<PathBuf>,
    native_graph: Option<PathBuf>,
    native_index: Option<PathBuf>,
    native_blobs: Option<PathBuf>,
    source_inputs: Vec<PathBuf>,
    exact_names: Vec<String>,
    output: PathBuf,
    explicit_bake_workspace: PathBuf,
    rustc: PathBuf,
) -> CliResult<ExitCode> {
    execute(
        compiler_root,
        target,
        declaration,
        native_graph,
        native_index,
        native_blobs,
        source_inputs,
        exact_names,
        output,
        explicit_bake_workspace,
        rustc,
    )
    .map_err(|error| CliError::failure(format!("native compiler tests: {error}")))
}

/// Validate root containment and its authored dependency contract before acquiring any execution plan.
#[allow(clippy::too_many_arguments)]
fn execute(
    compiler_root: PathBuf,
    target: PathBuf,
    declaration: Option<PathBuf>,
    native_graph: Option<PathBuf>,
    native_index: Option<PathBuf>,
    native_blobs: Option<PathBuf>,
    source_inputs: Vec<PathBuf>,
    exact_names: Vec<String>,
    output: PathBuf,
    explicit_bake_workspace: PathBuf,
    rustc: PathBuf,
) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let started = Instant::now();
    let compiler_root = compiler_root.canonicalize()?;
    let source = validated_source(&compiler_root, &target)?;

    // ---- Source authority and ordinary native dependency selection ----
    let tests = source.parent().ok_or("test source has no parent")?;
    let owner = tests.parent().ok_or("test source has no package owner")?;
    let declaration = validated_input(
        &compiler_root,
        &declaration.unwrap_or_else(|| source.with_extension("loaf.toml")),
    )?;
    let manifest = ProjectManifest::load(&declaration)?;
    let declaration_owner = declaration
        .parent()
        .ok_or("test declaration has no owner")?
        .to_path_buf();
    let mut source_inputs = source_inputs
        .iter()
        .map(|input| validated_input(&compiler_root, input))
        .collect::<Result<Vec<_>, _>>()?;
    source_inputs.push(declaration);
    source_inputs.sort();
    source_inputs.dedup();
    let project = manifest
        .project
        .as_ref()
        .ok_or("test declaration has no project identity")?;
    let features = compilation_features(&manifest)?;
    let compile_environment = compilation_environment(owner)?;
    let receipt = test_receipt(
        &compiler_root,
        &source,
        tests,
        project,
        &rustc,
        &features,
        &source_inputs,
        &compile_environment,
    )?;
    std::fs::create_dir_all(&output)?;
    let store = OvenStore::new(
        output
            .parent()
            .ok_or("test output has no parent")?
            .join("native-compiler-test-store"),
        OvenStoreLimits::new(
            oven_store::DEFAULT_OVEN_COMPILER_SUITE_MAX_PHYSICAL_BYTES,
            oven_store::DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_PHYSICAL_BYTES,
            oven_store::DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_LOGICAL_BYTES,
        ),
    );
    let dependencies = manifest.rust_dependencies().values().cloned().collect::<Vec<_>>();
    let prepared = if dependencies.is_empty() {
        None
    } else {
        Some(preparation::prepare(
            &compiler_root,
            native_graph
                .as_deref()
                .ok_or("native dependencies require --native-graph")?,
            native_index
                .as_deref()
                .ok_or("native dependencies require --native-index")?,
            native_blobs
                .as_deref()
                .ok_or("native dependencies require --native-blobs")?,
            &rustc,
        )?)
    };
    let closure = if let Some(prepared) = &prepared {
        let roots = prepared
            .graph()
            .select_dependency_roots(&dependencies, &declaration_owner, "target")?;
        prepared.graph().select(&roots)?
    } else {
        NativeLoafGraph::default().select(&[])?
    };
    let (receipt, selection) =
        incan_driver::build::native_loaf_plan::select_native_loaf_plan(&store, &receipt, &closure)?;
    preparation::write_report(&output, prepared.as_ref(), &closure)?;
    let plan_ms = started.elapsed().as_millis();

    // ---- Receipt-bound test compilation ----
    let mut artifacts = selection.artifacts().clone();
    artifacts.compile_environment.extend(compile_environment.clone());
    let mut plan = selection.source_artifact_plan("test-root")?;
    plan.compile_environment.extend(compile_environment);
    let executable = output.join("native-libtest");
    let compilation_started = Instant::now();
    let crate_name = source
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("test crate name is not UTF-8")?;
    let baked = bake_trusted_direct_rustc_test_in_store(
        &OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &artifacts,
            artifact_root: selection.output_guard_root(),
            artifact_plan: Some(&plan),
            rustc: &rustc,
            source: &source,
            output: &executable,
            crate_name,
            edition: "2024",
            source_evidence_key: "test-root",
            features: &features,
            prefer_dynamic: false,
        },
        &store,
    )?;
    let compilation_ms = compilation_started.elapsed().as_millis();

    // ---- Inventory-verified native execution ----
    let environment = execution_environment(&compiler_root, owner, &explicit_bake_workspace, &rustc)?;
    execute_cases(
        &executable,
        &exact_names,
        &environment,
        owner,
        &output,
        plan_ms,
        compilation_ms,
        baked.reused,
        &features,
    )?;
    Ok(ExitCode::SUCCESS)
}

/// Resolve the requested root before any output exists, rejecting traversal and symlink escapes from the checkout.
fn validated_source(root: &Path, target: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let source = root.join(target).canonicalize()?;
    if !source.starts_with(root) || source.extension().is_none_or(|extension| extension != "rs") {
        return Err("test target must be a Rust source inside the selected compiler checkout".into());
    }
    Ok(source)
}

/// Admit explicit source evidence only inside the checkout whose test compilation it will authorize.
fn validated_input(root: &Path, input: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let input = root.join(input).canonicalize()?;
    if !input.starts_with(root) {
        return Err("test input must stay inside the selected compiler checkout".into());
    }
    Ok(input)
}

/// Resolve local unit cfg features with the package feature authority; dependency features remain authored Rust inputs.
fn compilation_features(manifest: &ProjectManifest) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let selected = incan_provider::PackageFeatureGraph::from_manifest(manifest)?
        .resolve(&incan_provider::FeatureSelection::default())?;
    if !selected.active_optional_dependencies.is_empty()
        || !selected.dependency_features.is_empty()
        || !selected.required_sdk_components.is_empty()
    {
        return Err(
            "native test cfg features require local includes; declare Rust dependency features in [rust.dependencies]"
                .into(),
        );
    }
    Ok(selected.active_features.into_iter().collect())
}

/// Bind the test's full module closure and explicit compilation inputs before native dependency selection.
#[allow(clippy::too_many_arguments)]
fn test_receipt(
    root: &Path,
    source: &Path,
    tests: &Path,
    project: &oven_model::manifest::ProjectSection,
    rustc: &Path,
    features: &[String],
    inputs: &[PathBuf],
    environment: &BTreeMap<String, String>,
) -> Result<oven_store::OvenReceipt, Box<dyn std::error::Error>> {
    let mut request = oven_store::OvenGeneratedProjectRequest::new(
        root,
        project.name.as_deref().ok_or("test declaration has no project name")?,
        project
            .version
            .as_deref()
            .ok_or("test declaration has no project version")?,
        oven_rustc::rustc::rustc_host_target(rustc)?,
        oven_rustc::rustc::rustc_identity(rustc)?,
        "debug",
        features.to_vec(),
    )
    .with_generated_source("test-root", source)
    .with_generated_source_tree("test-modules", tests)
    .with_build_unit_input("test-environment", serde_json::to_string(environment)?);
    for input in inputs {
        let relative = input.strip_prefix(root)?.to_str().ok_or("test input is not UTF-8")?;
        let role = format!("test-input:{relative}");
        let metadata = std::fs::metadata(input)?;
        request = if metadata.is_dir() {
            request.with_generated_source_tree(role, input)
        } else if metadata.is_file() {
            request.with_generated_source(role, input)
        } else {
            return Err("test input must be a regular file or directory".into());
        };
    }
    oven_store::receipt_generated_project(&request).map_err(Into::into)
}

/// Provide package anchors required by compiler test macros without admitting ambient Cargo state.
fn compilation_environment(owner: &Path) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let path = |path: &Path| path.to_str().map(str::to_string).ok_or("test path is not UTF-8");
    Ok(BTreeMap::from([("CARGO_MANIFEST_DIR".into(), path(owner)?)]))
}

/// Supply the runtime anchors and explicit debug fixture policy after the native runner clears Cargo variables.
fn execution_environment(
    root: &Path,
    owner: &Path,
    workspace: &Path,
    rustc: &Path,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let mut environment = compilation_environment(owner)?;
    let compiler = std::env::current_exe()?.with_file_name("incan");
    for (name, path) in [
        ("CARGO_BIN_EXE_incan", compiler.as_path()),
        ("INCAN_SOURCE_ROOT", root),
        ("RUSTC", rustc),
        ("INCAN_INTERNAL_TEST_SOURCE_ROOT", root),
        ("INCAN_INTERNAL_OVEN_EXPLICIT_BAKE_WORKSPACE", workspace),
    ] {
        environment.insert(
            name.into(),
            path.to_str().ok_or("test runtime path is not UTF-8")?.into(),
        );
    }
    environment.insert("INCAN_OVEN_BAKE_PROFILES".into(), "debug".into());
    environment.insert("INCAN_TEST_COMMAND_TIMINGS".into(), "1".into());
    Ok(environment)
}

/// Verify selected case names against the real libtest inventory, then retain complete outcomes and phase timings.
#[allow(clippy::too_many_arguments)]
fn execute_cases(
    executable: &Path,
    exact_names: &[String],
    environment: &BTreeMap<String, String>,
    owner: &Path,
    output: &Path,
    plan_ms: u128,
    compilation_ms: u128,
    reused: bool,
    features: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let report = if exact_names.is_empty() {
        oven_rustc::native_test::run_native_test_batch_all_for_request(
            &oven_rustc::native_test::OvenNativeTestBatchRequest {
                executable,
                environment,
                working_directory: Some(owner),
                timeout: None,
                test_threads: None,
                root_label: Some("native compiler tests"),
                case_slice: None,
                progress: None,
            },
        )?
    } else {
        oven_rustc::native_test::run_native_tests_exact_in_directory_with_timeout(
            executable,
            exact_names,
            environment,
            Some(owner),
            None,
            Some("native compiler tests"),
        )?
    };
    std::fs::write(output.join("native-test.log"), &report.output)?;
    std::fs::write(
        output.join("compiler-suite-report.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "incan.oven.native-compiler-tests/1",
            "selection": if exact_names.is_empty() { "complete-root" } else { "exact-diagnostic" },
            "success": report.success,
            "case_counts": report.case_counts,
            "native_command_timings": report.command_timings,
            "timings_ms": {"native_plan": plan_ms, "test_compilation": compilation_ms,
                "inventory": report.timing.inventory_elapsed_ms, "execution": report.timing.execution_elapsed_ms},
            "test_binary_reused": reused,
            "unit_features": features,
        }))?,
    )?;
    print!("{}", report.output);
    if !report.success {
        return Err("selected native test failed; retained native-test.log contains the transcript".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A caller cannot turn a checkout-relative target into a test source outside its declared source authority.
    #[test]
    fn target_cannot_escape_compiler_checkout() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = tempfile::tempdir()?;
        let root = fixture.path().join("checkout");
        std::fs::create_dir_all(root.join("tests"))?;
        std::fs::write(root.join("tests/root.rs"), "#[test] fn case() {}")?;
        std::fs::write(fixture.path().join("outside.rs"), "#[test] fn case() {}")?;
        let root = root.canonicalize()?;
        assert_eq!(
            validated_source(&root, Path::new("tests/root.rs"))?,
            root.join("tests/root.rs")
        );
        assert!(validated_source(&root, Path::new("../outside.rs")).is_err());
        Ok(())
    }

    /// Changing an imported test module invalidates the executable even when the root's own bytes are unchanged.
    #[test]
    fn receipt_binds_complete_test_module_tree() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let tests = root.path().join("tests");
        std::fs::create_dir_all(&tests)?;
        let source = tests.join("root.rs");
        std::fs::write(&source, "mod helper; #[test] fn case() { assert!(helper::value()); }")?;
        std::fs::write(tests.join("helper.rs"), "pub fn value() -> bool { true }")?;
        let project = oven_model::manifest::ProjectSection {
            name: Some("fixture".into()),
            version: Some("0.1.0".into()),
            ..Default::default()
        };
        let rustc = oven_rustc::rustc::resolve_active_rustc()?;
        let environment = BTreeMap::new();
        let first = test_receipt(root.path(), &source, &tests, &project, &rustc, &[], &[], &environment)?;
        std::fs::write(tests.join("helper.rs"), "pub fn value() -> bool { false }")?;
        let second = test_receipt(root.path(), &source, &tests, &project, &rustc, &[], &[], &environment)?;
        assert_ne!(first.identity, second.identity);
        first.verify_identity()?;
        second.verify_identity()?;
        Ok(())
    }
}
