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
use incan_driver::oven_store::{OvenStoreCommandOptions, open_store_with_defaults};
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
    store: OvenStoreCommandOptions,
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
        store,
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
    store_options: OvenStoreCommandOptions,
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
    let selected_features = compilation_features(&manifest)?;
    let features = selected_features.active_features.iter().cloned().collect::<Vec<_>>();
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
    let store = native_test_store(store_options)?;
    let effective = selected_test_manifest(
        &manifest,
        &mut incan_driver::build::executable::ExecutableBuildOwner::default(),
        &rustc,
        &receipt.intent.target,
    )?;
    let dependencies = native_test_dependencies(&effective, &selected_features)?;
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
            &dependencies,
            &declaration_owner,
            &store,
        )?)
    };
    let empty = NativeLoafGraph::default().select(&[])?;
    let closure = prepared.as_ref().map_or(&empty, |prepared| prepared.closure());
    let (receipt, selection) =
        incan_driver::build::native_loaf_plan::select_native_loaf_plan(&store, &receipt, closure)?;
    preparation::write_report(&output, prepared.as_ref(), closure)?;
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
    let mut environment = execution_environment(&compiler_root, owner, &explicit_bake_workspace, &rustc)?;
    super::suite_environment::append_compiler_suite_direct_rustc_environment(
        &mut environment,
        &[oven_store::compiler_suite_env::OVEN_COMPILER_SUITE_CAPABILITY_ENV],
        &rustc,
        &plan,
    )?;
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

/// Project conditional alternatives only after the shared owner evaluates every original condition.
/// The source manifest stays unchanged; this command-local effective view carries only exact selected edges.
/// Missing decisions and incompatible simultaneously active alternatives refuse instead of silently dropping edges.
pub(crate) fn selected_test_manifest(
    manifest: &ProjectManifest,
    owner: &mut incan_driver::build::executable::ExecutableBuildOwner,
    rustc: &Path,
    target: &str,
) -> Result<ProjectManifest, Box<dyn std::error::Error>> {
    let conditions = manifest
        .loaf_dependencies
        .values()
        .flatten()
        .filter_map(|edge| edge.target.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if conditions.is_empty() {
        return Ok(manifest.clone());
    }
    let declaration = format!("{}:{:?}", manifest.path().display(), manifest.loaf_dependencies);
    let decisions = owner.target_predicates(&declaration, rustc, target, &conditions)?;
    let decisions = conditions.into_iter().zip(decisions).collect::<BTreeMap<_, _>>();
    project_selected_targets(manifest, &decisions)
}

/// Apply checked booleans to the original alternatives and reuse the existing effective-manifest projection.
fn project_selected_targets(
    manifest: &ProjectManifest,
    decisions: &BTreeMap<String, bool>,
) -> Result<ProjectManifest, Box<dyn std::error::Error>> {
    let mut rust = manifest.rust_dependencies().clone();
    let mut libraries = manifest.library_dependencies().clone();
    let mut selected = manifest.loaf_dependencies.clone();
    for (alias, alternatives) in &manifest.loaf_dependencies {
        if !alternatives.iter().any(|edge| edge.target.is_some()) {
            continue;
        }
        rust.remove(alias);
        libraries.remove(alias);
        selected.remove(alias);
        let mut active: Option<oven_model::manifest::LoafDependency> = None;
        for edge in alternatives {
            if let Some(condition) = &edge.target
                && !*decisions
                    .get(condition)
                    .ok_or_else(|| format!("missing evaluated target condition for {alias}"))?
            {
                continue;
            }
            if let Some(current) = &mut active {
                if current.loaf != edge.loaf
                    || current.spec.source != edge.spec.source
                    || current.spec.version != edge.spec.version
                    || current.spec.package != edge.spec.package
                {
                    return Err(format!("conditional dependency {alias} has incompatible active alternatives").into());
                }
                current.spec.features.extend(edge.spec.features.iter().cloned());
                current.spec.features.sort();
                current.spec.features.dedup();
                current.spec.default_features |= edge.spec.default_features;
                current.spec.optional &= edge.spec.optional;
            } else {
                let mut projected = edge.clone();
                projected.target = None;
                active = Some(projected);
            }
        }
        if let Some(edge) = active {
            rust.insert(alias.clone(), edge.spec.clone());
            selected.insert(alias.clone(), vec![edge]);
        }
    }
    let mut effective = manifest.with_effective_dependencies(libraries, rust, manifest.rust_dev_dependencies().clone());
    effective.loaf_dependencies = selected;
    Ok(effective)
}

/// Project declared Loaves into the explicit Rust test unit's native requests, including mixed Incan/Rust Loaves.
/// The existing native selector must validate each actual Rust facet and its original source declaration. A parsed
/// Incan dependency is not itself native authority, and a Loaf without a selected Rust facet must still be refused.
pub(crate) fn native_test_dependencies(
    manifest: &ProjectManifest,
    features: &incan_provider::ResolvedPackageFeatures,
) -> Result<Vec<oven_model::manifest::DependencySpec>, Box<dyn std::error::Error>> {
    let mut resolved = incan_provider::dependency_resolver::resolve_dependencies(
        Some(manifest),
        &[],
        true,
        &oven_model::lock::CargoFeatureSelection::default().normalized(),
    )
    .map_err(|errors| format!("invalid Rust test dependencies: {errors:?}"))?;
    resolved
        .apply_package_features(manifest, features)
        .map_err(|errors| format!("invalid Rust test feature projection: {errors:?}"))?;
    let mut dependencies = resolved
        .dependencies
        .into_iter()
        .chain(resolved.dev_dependencies)
        .map(|dependency| (dependency.crate_name.clone(), dependency))
        .collect::<BTreeMap<_, _>>();
    for (alias, library) in manifest.library_dependencies() {
        if library.optional && !features.active_optional_dependencies.contains(alias) {
            continue;
        }
        let mut requested = library.features.clone();
        requested.extend(features.dependency_features.get(alias).into_iter().flatten().cloned());
        requested.sort();
        requested.dedup();
        let dependency = oven_model::manifest::DependencySpec {
            crate_name: alias.clone(),
            version: None,
            features: requested,
            default_features: library.default_features,
            source: oven_model::manifest::DependencySource::Path {
                path: library.path.clone(),
            },
            optional: library.optional,
            package: None,
        };
        if dependencies.insert(alias.clone(), dependency).is_some() {
            return Err(format!("native test dependency `{alias}` has competing declarations").into());
        }
    }
    Ok(dependencies.into_values().collect())
}

/// Share the ordinary home Store across checkouts and caller projections, honoring explicit Store policy.
fn native_test_store(options: OvenStoreCommandOptions) -> CliResult<OvenStore> {
    open_store_with_defaults(
        &options,
        OvenStoreLimits::new(
            oven_store::DEFAULT_OVEN_COMPILER_SUITE_MAX_PHYSICAL_BYTES,
            oven_store::DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_PHYSICAL_BYTES,
            oven_store::DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_LOGICAL_BYTES,
        ),
    )
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
fn compilation_features(
    manifest: &ProjectManifest,
) -> Result<incan_provider::ResolvedPackageFeatures, Box<dyn std::error::Error>> {
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
    Ok(selected)
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

/// Supply runtime anchors, the canonical compiler stack and debug fixture policy after clearing Cargo variables.
/// Libtest executes compiler work on its own threads, so it must receive the same stack policy as CLI and LSP work.
fn execution_environment(
    root: &Path,
    owner: &Path,
    workspace: &Path,
    rustc: &Path,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let mut environment = compilation_environment(owner)?;
    let compiler = runtime_compiler(&std::env::current_exe()?);
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
    environment.insert(
        "RUST_MIN_STACK".into(),
        incan_frontend::compiler_stack::compiler_stack_bytes().to_string(),
    );
    Ok(environment)
}

/// Preserve the actual Incan compiler selected for this command, including versioned development candidates.
/// Only the standalone Oven executable needs its adjacent Incan entrypoint; substituting a sibling for an Incan
/// command can silently execute a different compiler than the one whose native test preparation is being measured.
fn runtime_compiler(executable: &Path) -> PathBuf {
    match executable.file_name().and_then(|name| name.to_str()) {
        Some("oven") => executable.with_file_name("incan"),
        Some("oven.exe") => executable.with_file_name("incan.exe"),
        _ => executable.to_path_buf(),
    }
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

    /// Keep physical fixture ownership alive while the normal manifest parser supplies conditional declarations.
    fn conditional_manifest(
        dependencies: &str,
    ) -> Result<(tempfile::TempDir, ProjectManifest), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("loaf.toml");
        std::fs::write(
            &path,
            format!("[project]\nname='conditional_fixture'\nversion='0.1.0'\n[dependencies]\n{dependencies}"),
        )?;
        let manifest = ProjectManifest::load(&path)?;
        Ok((root, manifest))
    }

    /// Every false edge remains in the source but disappears from the command-local native request.
    #[test]
    fn target_projection_keeps_source_and_rejects_missing_decisions() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, manifest) =
            conditional_manifest("web_time = {loaf='crates-io/web-time',version='1',target='cfg(wasm)'}\n")?;
        assert!(project_selected_targets(&manifest, &BTreeMap::new()).is_err());
        let false_view = project_selected_targets(&manifest, &BTreeMap::from([("cfg(wasm)".into(), false)]))?;
        assert!(!false_view.rust_dependencies().contains_key("web_time"));
        assert!(!false_view.loaf_dependencies.contains_key("web_time"));
        assert_eq!(
            manifest.loaf_dependencies["web_time"][0].target.as_deref(),
            Some("cfg(wasm)")
        );
        let true_view = project_selected_targets(&manifest, &BTreeMap::from([("cfg(wasm)".into(), true)]))?;
        assert!(true_view.rust_dependencies().contains_key("web_time"));
        assert!(true_view.loaf_dependencies["web_time"][0].target.is_none());
        Ok(())
    }

    /// Simultaneous compatible declarations union features/defaults, while incompatible source choices refuse.
    #[test]
    fn target_projection_merges_only_compatible_alternatives() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, manifest) = conditional_manifest(
            "dep = [{loaf='crates-io/serde',version='1',target='cfg(unix)',features=['derive'],default-features=false},{loaf='crates-io/serde',version='1',target='cfg(macos)',features=['std']}]\n",
        )?;
        let decisions = BTreeMap::from([("cfg(unix)".into(), true), ("cfg(macos)".into(), true)]);
        let selected = project_selected_targets(&manifest, &decisions)?;
        let spec = selected
            .rust_dependencies()
            .get("dep")
            .ok_or("missing selected alias")?;
        assert_eq!(spec.features, ["derive", "std"]);
        assert!(spec.default_features);
        let mut conflict = manifest.clone();
        conflict
            .loaf_dependencies
            .get_mut("dep")
            .ok_or("missing alternatives")?[1]
            .spec
            .version = Some("2".into());
        assert!(project_selected_targets(&conflict, &decisions).is_err());
        Ok(())
    }

    /// A versioned source compiler must survive the runtime handoff even if an older sibling `incan` exists.
    #[test]
    fn native_runtime_preserves_the_selected_compiler_entrypoint() {
        let selected = Path::new("/toolchain/bin/incan-coreadmission-3bf35cc281e9");
        assert_eq!(runtime_compiler(selected), selected);
        assert_eq!(
            runtime_compiler(Path::new("/toolchain/bin/oven")),
            Path::new("/toolchain/bin/incan")
        );
        assert_eq!(
            runtime_compiler(Path::new("/toolchain/bin/oven.exe")),
            Path::new("/toolchain/bin/incan.exe")
        );
    }

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

    /// Compatible checkouts retain one native dependency generation in the command's selected Store.
    #[test]
    fn native_dependencies_share_test_store_across_checkouts() -> Result<(), Box<dyn std::error::Error>> {
        const CHILD: &str = "INCAN_NATIVE_TEST_STORE_SHARING_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "commands::oven::native_compiler_tests::tests::native_dependencies_share_test_store_across_checkouts",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                // The outer repeat requires its test executable to be reused. This control owns a fresh Store;
                // its first/repeat assertions below prove the separate cold-publication and warm-reuse contract.
                .env_remove("INCAN_TEST_REQUIRE_STORED_NATIVE_REUSE")
                .output()?;
            assert!(
                output.status.success(),
                "native Store sharing child failed: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(());
        }
        let fixture = tempfile::tempdir()?;
        let root = fixture.path().canonicalize()?;
        let shared_store = root.join("shared-store");
        let rustc = oven_rustc::rustc::resolve_active_rustc()?;
        for checkout in ["one", "two"] {
            let checkout = root.join(checkout);
            std::fs::create_dir_all(checkout.join("dependency/src"))?;
            std::fs::create_dir_all(checkout.join("dependency/native/src"))?;
            std::fs::create_dir_all(checkout.join("tests"))?;
            std::fs::write(
                checkout.join("dependency/loaf.toml"),
                "[project]\nname='dependency'\nversion='1.0.0'\n[rust]\nname='dependency'\ntype='lib'\nedition='2024'\n[rust.source]\nroot='native'\n",
            )?;
            std::fs::write(
                checkout.join("dependency/src/lib.incn"),
                "pub def value() -> int:\n    return 7\n",
            )?;
            std::fs::write(
                checkout.join("dependency/native/src/lib.rs"),
                "pub const VALUE: u8 = 7;\n",
            )?;
            std::fs::write(
                checkout.join("tests/root.loaf.toml"),
                "[project]\nname='shared-native-test'\nversion='1.0.0'\n[dependencies]\ndependency={loaf='dependency',path='../dependency'}\n",
            )?;
            std::fs::write(
                checkout.join("tests/root.rs"),
                "#[test] fn case() { assert_eq!(dependency::VALUE, 7); }\n",
            )?;
            std::fs::write(
                checkout.join("lock.json"),
                r#"{"schema":"incan.oven.loaf-resolution/2","units":[]}"#,
            )?;
            std::fs::write(
                checkout.join("graph.json"),
                r#"{"index_commit":"0000000000000000000000000000000000000000","registry_lock":"lock.json","facets":[{"project":"dependency","features":[],"domain":"target"}]}"#,
            )?;
        }
        for phase in ["first", "repeat"] {
            for name in ["one", "two"] {
                let checkout = root.join(name);
                let output = checkout.join(format!("outputs/{phase}"));
                execute(
                    checkout.clone(),
                    PathBuf::from("tests/root.rs"),
                    None,
                    Some(checkout.join("graph.json")),
                    Some(root.clone()),
                    Some(root.clone()),
                    Vec::new(),
                    vec!["case".into()],
                    output.clone(),
                    OvenStoreCommandOptions {
                        root: Some(shared_store.clone()),
                        max_physical_bytes: Some(64 * 1024 * 1024),
                        max_domain_physical_bytes: Some(64 * 1024 * 1024),
                        max_domain_logical_bytes: Some(64 * 1024 * 1024),
                    },
                    root.join("fixtures"),
                    rustc.clone(),
                )?;
                let report: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(output.join("compiler-suite-report.json"))?)?;
                assert_eq!(report["success"], true);
                assert_eq!(report["test_binary_reused"], phase == "repeat");
                let preparation: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(output.join("native-loaf-preparation.json"))?)?;
                assert_eq!(
                    preparation["native"]["compiled"]
                        .as_array()
                        .ok_or("missing native work")?
                        .len(),
                    usize::from(phase == "first" && name == "one")
                );
                if phase == "repeat" {
                    assert_eq!(preparation["native"]["preparation_calls"], 0);
                }
                assert!(!checkout.join("target/compiler-development/native-loafs/store").exists());
            }
        }
        let store = OvenStore::new(
            shared_store,
            OvenStoreLimits::new(64 * 1024 * 1024, 64 * 1024 * 1024, 64 * 1024 * 1024),
        );
        let inspection = store.inspect()?;
        for domain in ["sdk-source-unit-target", "native-loaf-record"] {
            assert_eq!(
                inspection
                    .entries
                    .iter()
                    .filter(|entry| entry.manifest.domain == domain)
                    .count(),
                1
            );
        }
        Ok(())
    }

    /// Separate checkouts, roots and output projections share bounded retention with exact executable reuse.
    #[test]
    fn native_test_roots_share_store_and_reuse_across_outputs() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = tempfile::tempdir()?;
        let root = fixture.path().canonicalize()?;
        let shared_store = root.join("shared-store");
        for checkout in ["checkout-one", "checkout-two"] {
            let sources = root.join(checkout).join("tests");
            std::fs::create_dir_all(&sources)?;
            for name in ["one", "two"] {
                std::fs::write(
                    sources.join(format!("{name}.rs")),
                    "#[test] fn case() { assert_eq!(2 + 2, 4); }\n",
                )?;
                std::fs::write(
                    sources.join(format!("{name}.loaf.toml")),
                    format!("[project]\nname = 'native-test-{name}'\nversion = '1.0.0'\n"),
                )?;
            }
        }
        let rustc = oven_rustc::rustc::resolve_active_rustc()?;
        let options = || OvenStoreCommandOptions {
            root: Some(shared_store.clone()),
            max_physical_bytes: Some(64 * 1024 * 1024),
            max_domain_physical_bytes: Some(64 * 1024 * 1024),
            max_domain_logical_bytes: Some(64 * 1024 * 1024),
        };
        for phase in ["first", "repeat"] {
            for checkout in ["checkout-one", "checkout-two"] {
                let checkout_root = root.join(checkout);
                for name in ["one", "two"] {
                    let output = checkout_root.join(format!("outputs/{phase}/{name}"));
                    execute(
                        checkout_root.clone(),
                        PathBuf::from(format!("tests/{name}.rs")),
                        None,
                        None,
                        None,
                        None,
                        Vec::new(),
                        vec!["case".into()],
                        output.clone(),
                        options(),
                        root.join("fixtures"),
                        rustc.clone(),
                    )?;
                    let report: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(output.join("compiler-suite-report.json"))?)?;
                    assert_eq!(report["success"], true);
                    assert_eq!(report["case_counts"]["passed"], 1);
                    assert_eq!(report["test_binary_reused"], phase == "repeat");
                    assert!(
                        !checkout_root
                            .join("target/compiler-development/native-test-store")
                            .exists()
                    );
                    assert!(
                        !output
                            .parent()
                            .ok_or("output has no parent")?
                            .join("native-compiler-test-store")
                            .exists()
                    );
                }
            }
        }
        let store = native_test_store(options())?;
        assert_eq!(store.root(), shared_store);
        let inspection = store.inspect()?;
        assert!(inspection.physical_bytes <= store.limits().max_physical_bytes);
        assert_eq!(
            inspection
                .entries
                .iter()
                .filter(|entry| entry.manifest.kind == oven_store::store::OvenArtifactKind::Engine)
                .count(),
            4
        );
        let defaults = OvenStoreCommandOptions {
            root: None,
            max_physical_bytes: None,
            max_domain_physical_bytes: None,
            max_domain_logical_bytes: None,
        };
        let shared = native_test_store(defaults.clone())?;
        let ordinary = incan_driver::oven_store::open_store(&defaults)?;
        assert_eq!(shared.root(), ordinary.root());
        Ok(())
    }
}
