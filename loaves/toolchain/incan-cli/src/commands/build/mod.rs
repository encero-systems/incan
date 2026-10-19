//! Build and run pipeline for Incan projects.
//!
//! `incan build` and `incan run` submit executable builds to the shared project owner, which reuses a completed
//! Oven output or builds through the direct route; `incan run` then executes the binary the build reports. Library
//! builds return as direct-route library units.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::commands::build_report::emit_workspace_build_report;
use crate::{CliError, CliResult, ExitCode};
use incan_driver::build::inline_command::{inline_command_project, wrap_inline_command_source};
use incan_driver::build::rust_extern::{RustExternBuildFailureKind, RustExternDeclContext};
use incan_driver::build::{BuildCommandOptions, RunProfile};
use incan_driver::build_report::BuildReportOptions;
use incan_driver::lock::policy::LockPolicy;
use incan_frontend::diagnostics;
use incan_provider::FeatureSelection;
use oven_rustc::rustc::clear_inherited_cargo_environment;

/// Print human build progress to stderr when stdout is reserved for a machine-readable report.
fn print_build_progress(report_options: &BuildReportOptions, message: impl AsRef<str>) {
    if report_options.enabled() {
        eprintln!("{}", message.as_ref());
    } else {
        println!("{}", message.as_ref());
    }
}

#[allow(dead_code)]
/// Classify a supported Rust compiler failure that mentions the declared item or its backing module.
fn classify_rust_extern_build_failure(
    stderr: &str,
    item_name: &str,
    rust_module_path: &str,
) -> Option<RustExternBuildFailureKind> {
    if !stderr.contains(item_name) && !stderr.contains(rust_module_path) {
        return None;
    }
    if stderr.contains("gated behind the")
        || stderr.contains("configured out")
        || stderr.contains("the item is gated behind")
    {
        return Some(RustExternBuildFailureKind::FeatureGatedBackingPath);
    }
    if stderr.contains("mismatched types") || stderr.contains("error[E0308]") {
        return Some(RustExternBuildFailureKind::SignatureMismatch);
    }
    if stderr.contains("cannot find")
        || stderr.contains("failed to resolve")
        || stderr.contains("unresolved import")
        || stderr.contains("error[E0425]")
    {
        return Some(RustExternBuildFailureKind::UnresolvedBackingItem);
    }
    None
}

#[allow(dead_code)]
/// Render deduplicated Incan diagnostics for recognized Rust extern build failures.
fn format_rust_extern_wrapped_diagnostics(stderr: &str, contexts: &[RustExternDeclContext]) -> Option<String> {
    let mut rendered = String::new();
    let mut seen: HashSet<String> = HashSet::new();
    for ctx in contexts {
        let Some(kind) = classify_rust_extern_build_failure(stderr, &ctx.item_name, &ctx.rust_module_path) else {
            continue;
        };
        let key = format!(
            "{}:{}:{}:{}",
            ctx.file_path.display(),
            ctx.item_name,
            ctx.span.start,
            ctx.span.end
        );
        if !seen.insert(key) {
            continue;
        }
        let err = match kind {
            RustExternBuildFailureKind::UnresolvedBackingItem => {
                diagnostics::errors::rust_extern_unresolved_backing_item(
                    &ctx.item_name,
                    &ctx.rust_module_path,
                    ctx.span,
                )
            }
            RustExternBuildFailureKind::SignatureMismatch => {
                diagnostics::errors::rust_extern_signature_mismatch(&ctx.item_name, &ctx.rust_module_path, ctx.span)
            }
            RustExternBuildFailureKind::FeatureGatedBackingPath => {
                diagnostics::errors::rust_extern_feature_gated_backing_path(
                    &ctx.item_name,
                    &ctx.rust_module_path,
                    ctx.span,
                )
            }
        };
        rendered.push_str(&diagnostics::format_error(
            ctx.file_path.to_string_lossy().as_ref(),
            &ctx.source,
            &err,
        ));
    }
    if rendered.is_empty() { None } else { Some(rendered) }
}


/// Build an Incan file to a native executable.
pub fn build_file(
    file_path: &str,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: BuildReportOptions,
) -> CliResult<ExitCode> {
    let report = build_file_report(file_path, output_dir, options, &report_options)?;
    emit_workspace_build_report(&report, &report_options)?;
    Ok(ExitCode::SUCCESS)
}

/// Submit an executable build to the shared owner; presentation remains local to this request.
pub(crate) fn build_file_report(
    file_path: &str,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: &BuildReportOptions,
) -> CliResult<serde_json::Value> {
    crate::project_owner::build(file_path, output_dir, options, |message| {
        print_build_progress(report_options, message);
    })
}

/// Refuse `incan build --lib` until libraries build as direct-route library units.
pub fn build_library(
    file_path: Option<&str>,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: BuildReportOptions,
) -> CliResult<ExitCode> {
    build_library_report(file_path, output_dir, options, &report_options).map(|_| ExitCode::SUCCESS)
}

/// Build one library project for workspace-level aggregation; refused until direct-route library units exist.
pub(crate) fn build_library_report(
    file_path: Option<&str>,
    _output_dir: Option<&String>,
    _options: BuildCommandOptions,
    _report_options: &BuildReportOptions,
) -> CliResult<incan_driver::build_report::BuildReport> {
    Err(CliError::failure(format!(
        "{}: `incan build --lib` does not build yet; libraries return as direct-route library units",
        file_path.unwrap_or(".")
    )))
}

/// Output format for `incan inspect backend-selection`.
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendSelectionInspectFormat {
    /// Human-readable summary of the receipt's key fields.
    Text,
    /// The complete receipt, pretty-printed.
    Json,
}

/// Read, verify, and render one persisted backend-selection execution receipt (#986).
///
/// Mirrors `inspect_oven_receipt`'s read-verify-render shape, but has no bounded store to consult: a backend-selection
/// receipt is self-contained, so verification is just `BackendExecutionReceipt::verify_identity`.
pub fn inspect_backend_selection(path: &Path, format: BackendSelectionInspectFormat) -> CliResult<ExitCode> {
    let bytes = fs::read(path).map_err(|error| {
        CliError::failure(format!(
            "failed to read backend-selection receipt {}: {error}",
            path.display()
        ))
    })?;
    let receipt = serde_json::from_slice::<incan_driver::backend::selection::BackendExecutionReceipt>(&bytes).map_err(
        |error| {
            CliError::failure(format!(
                "failed to parse backend-selection receipt {}: {error}",
                path.display()
            ))
        },
    )?;
    receipt
        .verify_identity()
        .map_err(|error| CliError::failure(error.to_string()))?;
    match format {
        BackendSelectionInspectFormat::Text => {
            println!("selected backend:   {:?}", receipt.selection.selected_backend);
            println!("executed backend:   {:?}", receipt.executed_backend);
            println!("backend revision:   {}", receipt.selection.implementation_revision);
            println!("compiler version:   {}", receipt.compiler_version);
            println!("selection identity: {}", receipt.selection.identity);
            println!("receipt identity:   {}", receipt.identity);
        }
        BackendSelectionInspectFormat::Json => {
            let json = serde_json::to_string_pretty(&receipt).map_err(|error| {
                CliError::failure(format!("failed to serialize backend-selection receipt: {error}"))
            })?;
            println!("{json}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Build and run an Incan file.
pub fn run_file(
    file_path: &str,
    lock_policy: LockPolicy,
    package_features: FeatureSelection,
    release: bool,
    program_args: Vec<String>,
) -> CliResult<ExitCode> {
    let report = crate::project_owner::build(file_path, None, run_options(lock_policy, package_features, release), |message| {
        eprintln!("{message}");
    })?;
    run_reported_executable(&report, &program_args)
}

/// Build and run inline Incan source from `incan run -c`.
pub fn run_inline_source(
    source: &str,
    lock_policy: LockPolicy,
    package_features: FeatureSelection,
    release: bool,
    program_args: Vec<String>,
) -> CliResult<ExitCode> {
    let wrapped_source = wrap_inline_command_source(source);
    let inline_project = inline_command_project(&wrapped_source)?;
    let source_path = inline_project.source_path;
    let source_parent = source_path.parent().ok_or_else(|| {
        CliError::failure(format!(
            "failed to determine temporary inline command directory for {}",
            source_path.display()
        ))
    })?;
    fs::create_dir_all(source_parent).map_err(|err| {
        CliError::failure(format!(
            "Error creating temporary inline command directory {}: {err}",
            source_parent.display()
        ))
    })?;
    let _inline_command_lock = oven_model::lock::acquire_publication_lock(&source_path).map_err(|error| {
        CliError::failure(format!(
            "failed to coordinate temporary inline command project {}: {error}",
            source_parent.display()
        ))
    })?;
    fs::write(&source_path, wrapped_source).map_err(|err| {
        CliError::failure(format!(
            "Error writing temporary inline command file {}: {err}",
            source_path.display()
        ))
    })?;
    let source_arg = source_path.to_string_lossy().to_string();
    let result = crate::project_owner::build(
        &source_arg,
        Some(&inline_project.output_dir),
        run_options(lock_policy, package_features, release),
        |message| eprintln!("{message}"),
    )
    .and_then(|report| run_reported_executable(&report, &program_args));
    let _ = fs::remove_file(&source_path);
    result
}

/// The executable build `incan run` requests: the debug profile unless `--release` asks otherwise.
fn run_options(lock_policy: LockPolicy, package_features: FeatureSelection, release: bool) -> BuildCommandOptions {
    BuildCommandOptions {
        executable_profile: if release { RunProfile::Release } else { RunProfile::Debug },
        lock_policy,
        package_features,
    }
}

/// Run the binary a successful executable build reported, from its project root, with the program's arguments.
///
/// The child gets no inherited Cargo state; its exit code becomes the command's exit code.
fn run_reported_executable(report: &serde_json::Value, program_args: &[String]) -> CliResult<ExitCode> {
    let binary = report["artifacts"]
        .as_array()
        .and_then(|artifacts| artifacts.iter().find(|artifact| artifact["kind"] == "binary"))
        .and_then(|artifact| artifact["path"].as_str())
        .map(PathBuf::from)
        .ok_or_else(|| CliError::failure("the executable build reported no binary artifact to run"))?;
    let mut command = Command::new(&binary);
    if let Some(project_root) = report["project"]["project_root"].as_str() {
        command.current_dir(Path::new(project_root));
    }
    clear_inherited_cargo_environment(&mut command);
    command.args(program_args);
    let status = command
        .status()
        .map_err(|error| CliError::failure(format!("failed to run {}: {error}", binary.display())))?;
    Ok(ExitCode(status.code().unwrap_or(ExitCode::FAILURE.0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use incan_driver::build::test_support::undesugared_vocab_declaration;
    use std::fs;
    use std::path::{Path, PathBuf};

    use incan_driver::build::library_exports::collect_library_rust_abi;
    use incan_driver::build::library_outputs::write_library_manifest_artifacts;
    use incan_driver::build::library_project::prepare_library_project;
    use incan_driver::build::rust_extern::{RustExternBuildFailureKind, RustExternDeclContext};
    use incan_driver::build::{BuildCommandOptions, OvenProjectPlanMode};
    use incan_driver::build_report::BuildReportOptions;
    use incan_driver::cargo_policy::CargoPolicy;
    use incan_driver::error::{CliError, ExitCode};
    use incan_frontend::api_metadata::ApiDeclaration;
    use incan_frontend::ast::{Declaration, Span};
    use incan_frontend::{body_ir, lexer, parser};

    use incan_frontend::library_manifest::LibraryManifest;
    use incan_provider::FeatureSelection;
    use oven_model::lock::{CargoFeatureSelection, IncanLock, compute_deps_fingerprint};

    #[cfg(feature = "rust_inspect")]
    use rust_inspect::Inspector;
    #[cfg(feature = "rust_inspect")]
    use rust_inspect::InspectorConfig;

    /// Copy authored component inputs without following symlinks or carrying generated build/cache directories.
    fn copy_source_tree(from: &Path, to: &Path) -> Result<(), Box<dyn std::error::Error>> {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            if matches!(entry.file_name().to_str(), Some("target" | ".git")) {
                continue;
            }
            let source = entry.path();
            let destination = to.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_source_tree(&source, &destination)?;
            } else if entry.file_type()?.is_file() {
                fs::copy(source, destination)?;
            }
        }
        Ok(())
    }

    /// #1976: SDK prewarm cache hits must not be the only check of `std.async`; its complete source graph must pass a
    /// fresh source check with an isolated SDK home.
    #[test]
    fn std_async_component_sources_typecheck_from_source_issue1976() -> Result<(), Box<dyn std::error::Error>> {
        const CHILD: &str = "INCAN_TEST_1976_STDLIB_SOURCE_CHILD";
        const ENTRY: &str = "INCAN_TEST_1976_STDLIB_SOURCE_ENTRY";
        if env::var_os(CHILD).is_some() {
            let entry = env::var(ENTRY)?;
            let report = incan_driver::build::check::check_path_report_with_selections(
                Path::new(&entry),
                &FeatureSelection::default(),
                None,
            )?;
            assert!(report.ok(), "async component source check failed: {report:?}");
            return Ok(());
        }

        let root = incan_test_support::repo_root();
        let state = tempfile::tempdir()?;
        let stdlib = state.path().join("stdlib");
        copy_source_tree(&root.join("loaves/stdlib"), &stdlib)?;
        let component = stdlib.join("async");
        let component_manifest = fs::read_to_string(component.join("loaf.toml"))?;
        let sdk_profile = component_manifest
            .find("\n[sdk]\n")
            .ok_or("async component manifest has no SDK profile")?;
        fs::write(component.join("loaf.toml"), &component_manifest[..sdk_profile])?;
        fs::write(
            component.join("rust/Cargo.toml"),
            "[package]\nname = \"incan_std_async\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\ntokio = { version = \"1\", features = [\"rt-multi-thread\", \"macros\", \"time\", \"sync\", \"net\"] }\n",
        )?;
        let entry = component.join("src/lib.incn");
        let child = std::process::Command::new(env::current_exe()?)
            .args([
                "--exact",
                "commands::build::tests::std_async_component_sources_typecheck_from_source_issue1976",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env(ENTRY, &entry)
            .env("INCAN_SOURCE_ROOT", &root)
            .env("INCAN_STDLIB", &stdlib)
            .env("INCAN_STDLIB_DIR", &stdlib)
            .env("INCAN_HOME", state.path().join("home"))
            .env(incan_provider::SDK_PROVIDER_BUILD_ENV, "stdlib-async")
            .env_remove(incan_provider::inventory::SDK_INVENTORY_OVERRIDE_ENV)
            .output()?;
        let stdout = String::from_utf8_lossy(&child.stdout);
        assert!(
            child.status.success(),
            "the std.async source-check child failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(stdout.contains("1 passed"), "the child ran no exact test:\n{stdout}");
        Ok(())
    }

    #[test]
    fn classify_signature_mismatch_for_rust_extern_context() {
        let stderr = "error[E0308]: mismatched types in `incan_std_testing::fail`\n  --> src/main.rs:10:5";
        let kind = classify_rust_extern_build_failure(stderr, "fail", "incan_std_testing");
        assert_eq!(kind, Some(RustExternBuildFailureKind::SignatureMismatch));
    }

    #[test]
    fn classify_unresolved_backing_item_for_rust_extern_context() {
        let stderr = "error[E0425]: cannot find function `fail` in module `incan_std_testing`";
        let kind = classify_rust_extern_build_failure(stderr, "fail", "incan_std_testing");
        assert_eq!(kind, Some(RustExternBuildFailureKind::UnresolvedBackingItem));
    }

    #[test]
    fn wraps_rust_extern_failure_back_to_incan_declaration_span() {
        let stderr = "error[E0425]: cannot find function `fail` in module `incan_std_testing`";
        let contexts = vec![RustExternDeclContext {
            file_path: PathBuf::from("stdlib/testing.incn"),
            source: "rust.module(\"incan_std_testing\")\n@rust.extern\ndef fail(msg: str) -> None:\n  ...\n"
                .to_string(),
            item_name: "fail".to_string(),
            rust_module_path: "incan_std_testing".to_string(),
            span: Span { start: 35, end: 73 },
        }];
        let rendered = format_rust_extern_wrapped_diagnostics(stderr, &contexts);
        let Some(rendered) = rendered else {
            panic!("expected wrapped diagnostic");
        };
        assert!(rendered.contains("Rust backing item"));
        assert!(rendered.contains("incan_std_testing::fail"));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn library_rust_abi_is_independent_of_partial_prewarm_cache_issue922() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let root = workspace.path().join("root");
        let dependency = workspace.path().join("source-dep");
        fs::create_dir_all(root.join("src"))?;
        fs::create_dir_all(dependency.join("src"))?;
        fs::write(
            root.join("loaf.toml"),
            "[project]\nname='root'\nversion='0.1.0'\n[rust]\nname='root'\ntype='lib'\nedition='2021'\n[dependencies]\nsource-dep={path='../source-dep'}\n",
        )?;
        fs::write(root.join("src/lib.rs"), "pub fn keep() {}\n")?;
        fs::write(
            dependency.join("loaf.toml"),
            "[project]\nname='source-dep'\nversion='0.1.0'\n[rust]\nname='source_dep'\ntype='lib'\nedition='2021'\n",
        )?;
        fs::write(
            dependency.join("src/lib.rs"),
            r#"
pub struct ChildId(String);

impl ChildId {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
"#,
        )?;

        // Both the root lookup and the dependency-local complete extraction use the existing direct source loader.
        // These authored fixtures have only local path edges and no registry closure to resolve.
        for manifest_dir in [&root, &dependency] {
            rust_inspect::write_oven_inspection_source_authority(manifest_dir, Vec::new())?;
            fs::write(manifest_dir.join(rust_inspect::OVEN_DIRECT_INSPECTION_MARKER), "1\n")?;
        }
        assert!(!root.join("Cargo.toml").exists());
        assert!(!dependency.join("Cargo.toml").exists());

        let query_path = "source_dep::ChildId".to_string();
        let inspector = Inspector::new(InspectorConfig::new(root.clone()));
        // Reproduce a persisted partial prewarm record explicitly: complete Oven extraction now eagerly includes
        // methods, but ABI publication must also upgrade records left by the source-only metadata path.
        inspector.cache().insert_test_item(
            &root,
            incan_lang::interop::RustItemMetadata {
                canonical_path: query_path.clone(),
                definition_path: Some(query_path.clone()),
                visibility: incan_lang::interop::RustVisibility::Public,
                kind: incan_lang::interop::RustItemKind::Type(incan_lang::interop::RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: incan_lang::interop::RustTypeMetadataCompleteness::FieldsAndVariantsOnly,
                    methods: Vec::new(),
                    implemented_traits: Vec::new(),
                    fields: Vec::new(),
                    variants: Vec::new(),
                }),
            },
        )?;
        inspector.cache().persist_manifest_dir(&root)?;
        inspector.prewarm([query_path.clone()], &|_| ())?;
        let prewarmed = inspector.get(&query_path)?;
        let incan_lang::interop::RustItemKind::Type(prewarmed_type) = &prewarmed.metadata.kind else {
            return Err("expected prewarmed ChildId type metadata".into());
        };
        assert!(
            !prewarmed_type.metadata_completeness.has_methods(),
            "the regression requires prewarm to reuse the persisted partial metadata"
        );

        let query_paths = vec![query_path.clone()];
        let cold = collect_library_rust_abi(&root, &query_paths)?.ok_or("expected cold library Rust ABI")?;
        inspector.cache().get_or_extract_complete(&root, &query_path, &|_| ())?;
        let warm = collect_library_rust_abi(&root, &query_paths)?.ok_or("expected warm library Rust ABI")?;

        assert_eq!(
            cold, warm,
            "library ABI publication must not depend on whether a previous compiler query upgraded the shared cache"
        );
        let child_id = warm.get(&query_path).ok_or("expected ChildId ABI item")?;
        let incan_lang::interop::RustItemKind::Type(child_id_type) = &child_id.kind else {
            return Err("expected ChildId ABI type metadata".into());
        };
        assert!(child_id_type.metadata_completeness.has_methods());
        assert!(child_id_type.methods.iter().any(|method| method.name == "as_str"));
        Ok(())
    }

    #[test]
    fn build_library_canonicalizes_explicit_and_implicit_nested_module_imports()
    -> Result<(), Box<dyn std::error::Error>> {
        if incan_test_support::in_compiler_source_layout(
            "commands::build::tests::build_library_canonicalizes_explicit_and_implicit_nested_module_imports",
        )? {
            return Ok(());
        }

        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(src_dir.join("dataset"))?;

        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"nestedlib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            src_dir.join("lib.incn"),
            "pub from dataset import DataSet\npub from dataset.mod import DataSet as ExplicitDataSet\npub from dataset.ops import filter_ds\n",
        )?;
        std::fs::write(
            src_dir.join("dataset").join("mod.incn"),
            "pub trait DataSet[T]:\n    pass\n",
        )?;
        std::fs::write(
            src_dir.join("dataset").join("ops.incn"),
            "from dataset.mod import DataSet\npub def filter_ds[T](ds: DataSet[T]) -> DataSet[T]:\n    return ds\n",
        )?;

        let cargo_lock_payload = std::fs::read_to_string(incan_test_support::repo_root().join("Cargo.lock"))?;
        let fingerprint = compute_deps_fingerprint(&[], &[], &CargoFeatureSelection::default(), Some(project_root));
        let incan_lock = IncanLock::new(
            incan_lang::version::INCAN_VERSION,
            fingerprint,
            CargoFeatureSelection::default(),
            cargo_lock_payload,
        );
        incan_lock.write(&project_root.join("oven.lock"))?;

        let lib_path = src_dir.join("lib.incn");
        let lib_path_str = lib_path
            .to_str()
            .ok_or("lib path should be valid utf-8 for build_library test")?;
        let exit = build_library(
            Some(lib_path_str),
            None,
            BuildCommandOptions::default(),
            BuildReportOptions::default(),
        )?;
        assert_eq!(exit, ExitCode::SUCCESS);

        let generated_lib = project_root.join("target").join("lib").join("src").join("lib.rs");
        let generated_dataset = project_root
            .join("target")
            .join("lib")
            .join("src")
            .join("dataset")
            .join("mod.rs");
        let generated_flat_dataset = project_root.join("target").join("lib").join("src").join("dataset.rs");

        let generated_lib_source = std::fs::read_to_string(&generated_lib)?;
        let generated_dataset_source = std::fs::read_to_string(&generated_dataset)?;

        assert!(
            !generated_lib_source.contains("crate::dataset::r#mod"),
            "generated lib.rs should not reference crate::dataset::r#mod"
        );
        assert!(
            !generated_dataset_source.contains("crate::dataset::r#mod"),
            "generated dataset/mod.rs should not reference crate::dataset::r#mod"
        );
        assert!(
            !generated_flat_dataset.exists(),
            "stale flat dataset.rs should not exist after nested library build"
        );

        Ok(())
    }

    #[test]
    fn build_library_publishes_an_executable_surface_joined_to_the_manifest_by_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        if incan_test_support::in_compiler_source_layout(
            "commands::build::tests::build_library_publishes_an_executable_surface_joined_to_the_manifest_by_identity",
        )? {
            return Ok(());
        }

        // RFC 123's core claim is that the manifest and the representation are joined by the identity space rather
        // than by file naming or ordering. This proves exactly that: the identity is taken from the manifest the
        // build published, hydrated, and looked up in the surface. Nothing here reconstructs an identity or matches
        // on a spelling, because a consumer is forbidden from doing either.
        use incan_semantics_core::SymbolOrigin;
        use incan_semantics_core::executable_representation::SurfaceReader;

        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"surfacelib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            src_dir.join("lib.incn"),
            "pub def doubled(value: int) -> int:\n    return value * 2\n",
        )?;

        let cargo_lock_payload = std::fs::read_to_string(incan_test_support::repo_root().join("Cargo.lock"))?;
        let fingerprint = compute_deps_fingerprint(&[], &[], &CargoFeatureSelection::default(), Some(project_root));
        IncanLock::new(
            incan_lang::version::INCAN_VERSION,
            fingerprint,
            CargoFeatureSelection::default(),
            cargo_lock_payload,
        )
        .write(&project_root.join("oven.lock"))?;

        let lib_path = src_dir.join("lib.incn");
        let lib_path_str = lib_path.to_str().ok_or("lib path should be valid utf-8")?;
        assert_eq!(
            build_library(
                Some(lib_path_str),
                None,
                BuildCommandOptions::default(),
                BuildReportOptions::default(),
            )?,
            ExitCode::SUCCESS
        );

        let manifest_path = project_root.join("target").join("lib").join("surfacelib.incnlib");
        let manifest = LibraryManifest::read_from_path(&manifest_path)?;
        let published = manifest
            .contract_metadata
            .identity_graph
            .function_identities_for_public_name("doubled")
            .into_iter()
            .flatten()
            .next()
            .ok_or("the manifest must publish a canonical identity for the exported function")?;

        // The module path comes from the published identity, never from the source file's name. A consumer has only
        // the identity, so a test that hardcodes a path tests something no consumer can do — and would still pass
        // against a producer that wrote the file somewhere else entirely.
        let SymbolOrigin::Package { .. } = &published.origin else {
            return Err("a published library export must carry a package origin".into());
        };
        let surface_path =
            incan_frontend::library_manifest::published_layout::executable_surface_path(&manifest_path, &manifest)
                .ok_or("the surface path must be derivable from the manifest path")?;
        assert!(
            surface_path.is_file(),
            "a library build must publish its executable representation at {}",
            surface_path.display()
        );

        let bytes = std::fs::read(&surface_path)?;
        let reader = SurfaceReader::open(&bytes)?;
        assert!(
            reader.covers(&published),
            "the surface must cover the identity the manifest published; it covers {:?}",
            reader.covered_identities().collect::<Vec<_>>()
        );
        assert_eq!(
            reader.declaration(&published)?.name,
            "doubled",
            "resolving the manifest's identity must reach the declaration it names"
        );
        Ok(())
    }

    /// Exercise the artifact-only `incan build --lib` publisher without mutating its process-wide internal mode flag.
    #[test]
    fn build_library_omits_private_generic_implementation_requirements_issue1280()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"privateimpl\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            src_dir.join("lib.incn"),
            r#"
"""Exercise a private generic implementation without publishing its compiler metadata."""

trait Copyable:
    def copy(self) -> Self: ...

model PrivateValue[T] with Copyable:
    value: T

    def copy(self) -> Self:
        return self

pub def answer() -> int:
    """Return the public library value."""
    return 42
"#,
        )?;

        let cargo_lock_payload = std::fs::read_to_string(incan_test_support::repo_root().join("Cargo.lock"))?;
        let fingerprint = compute_deps_fingerprint(&[], &[], &CargoFeatureSelection::default(), Some(project_root));
        let incan_lock = IncanLock::new(
            incan_lang::version::INCAN_VERSION,
            fingerprint,
            CargoFeatureSelection::default(),
            cargo_lock_payload,
        );
        incan_lock.write(&project_root.join("oven.lock"))?;

        let lib_path = src_dir.join("lib.incn");
        let lib_path_str = lib_path
            .to_str()
            .ok_or("lib path should be valid utf-8 for build_library test")?;
        let mut prepared = prepare_library_project(
            Some(lib_path_str),
            None,
            CargoPolicy::default(),
            &FeatureSelection::default(),
            None,
            Vec::new(),
            false,
            false,
            None,
            false,
            false,
            OvenProjectPlanMode::ConsumeOnly,
            None,
        )?;
        write_library_manifest_artifacts(&mut prepared)?;

        let manifest_path = project_root.join("target/lib/privateimpl.incnlib");
        let manifest = LibraryManifest::read_from_path(&manifest_path)?;
        let checked_api = manifest
            .contract_metadata
            .api
            .as_ref()
            .ok_or("library should publish checked API metadata")?;
        assert!(
            !checked_api
                .modules
                .iter()
                .flat_map(|module| &module.declarations)
                .any(|declaration| {
                    matches!(declaration, ApiDeclaration::Model(model) if model.name == "PrivateValue")
                }),
            "private implementation targets must not leak into checked API metadata"
        );
        assert!(
            !manifest.exports.models.iter().any(|model| model.name == "PrivateValue"),
            "private implementation targets must not leak into public model exports"
        );
        assert!(
            !std::fs::read_to_string(manifest_path)?.contains("PrivateValue"),
            "private implementation targets must not leak into the serialized library manifest"
        );

        Ok(())
    }

    #[test]
    fn build_library_publishes_public_registry_metadata_and_facade_projections()
    -> Result<(), Box<dyn std::error::Error>> {
        if incan_test_support::in_compiler_source_layout(
            "commands::build::tests::build_library_publishes_public_registry_metadata_and_facade_projections",
        )? {
            return Ok(());
        }

        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"registrylib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            src_dir.join("lib.incn"),
            r#"
from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
pub type FunctionId = newtype str

@derive(Descriptor)
pub model FunctionSpec:
    pub summary: str

pub static public_functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)
static private_functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)

@describe(public_functions, FunctionId("public"), FunctionSpec(summary="public"))
pub def public_function() -> None:
    pass

@describe(private_functions, FunctionId("private"), FunctionSpec(summary="private"))
def private_function() -> None:
    pass

pub from crate.feature import functions as public_feature_functions
pub from crate.feature import normalize as public_normalize
"#,
        )?;
        std::fs::write(
            src_dir.join("feature.incn"),
            r#"
from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
pub type FunctionId = newtype str

@derive(Descriptor)
pub model FunctionSpec:
    pub summary: str

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("normalize"), FunctionSpec(summary="Normalize text"))
pub def normalize(value: str) -> str:
    return value
"#,
        )?;

        let cargo_lock_payload = std::fs::read_to_string(incan_test_support::repo_root().join("Cargo.lock"))?;
        let fingerprint = compute_deps_fingerprint(&[], &[], &CargoFeatureSelection::default(), Some(project_root));
        let incan_lock = IncanLock::new(
            incan_lang::version::INCAN_VERSION,
            fingerprint,
            CargoFeatureSelection::default(),
            cargo_lock_payload,
        );
        incan_lock.write(&project_root.join("oven.lock"))?;

        let lib_path = src_dir.join("lib.incn");
        let lib_path_str = lib_path
            .to_str()
            .ok_or("lib path should be valid utf-8 for build_library test")?;
        let exit = build_library(
            Some(lib_path_str),
            None,
            BuildCommandOptions::default(),
            BuildReportOptions::default(),
        )?;
        assert_eq!(exit, ExitCode::SUCCESS);

        let manifest = LibraryManifest::read_from_path(&project_root.join("target/lib/registrylib.incnlib"))?;
        let registry = manifest
            .contract_metadata
            .registry
            .ok_or("library should publish checked registry metadata")?;
        assert_eq!(
            registry.schema_version,
            incan_frontend::registry_metadata::CHECKED_REGISTRY_METADATA_SCHEMA_VERSION
        );
        assert_eq!(
            registry.package,
            Some(incan_frontend::registry_metadata::CheckedRegistryPackageIdentity {
                name: "registrylib".to_string(),
                version: Some("0.1.0".to_string()),
            })
        );
        assert_eq!(registry.modules.len(), 2);
        let root = registry
            .modules
            .iter()
            .find(|module| module.module_path == ["lib".to_string()])
            .ok_or("root module should retain its explicit public registry facts")?;
        assert_eq!(root.registries.len(), 1);
        assert_eq!(root.registries[0].identity, "lib::public_functions");
        assert_eq!(root.entries.len(), 1);
        assert_eq!(root.entries[0].subject_identity, "lib.public_function");
        let feature = registry
            .modules
            .iter()
            .find(|module| module.module_path == ["feature".to_string()])
            .ok_or("feature module should retain canonical registry facts")?;
        assert_eq!(feature.registries.len(), 1);
        assert_eq!(feature.entries.len(), 1);
        assert_eq!(
            feature.registries[0]
                .reexport_paths
                .iter()
                .map(|projection| projection.path.clone())
                .collect::<Vec<_>>(),
            vec![vec!["lib".to_string(), "public_feature_functions".to_string()]]
        );
        assert_eq!(
            feature.entries[0]
                .reexport_paths
                .iter()
                .map(|projection| projection.path.clone())
                .collect::<Vec<_>>(),
            vec![vec!["lib".to_string(), "public_normalize".to_string()]]
        );
        assert!(
            !registry
                .modules
                .iter()
                .flat_map(|module| module.registries.iter())
                .any(
                    |definition| definition.identity.contains("__incan_std") || definition.identity.contains("private")
                ),
            "library artifact must contain only its explicit public registry surface"
        );
        Ok(())
    }

    #[test]
    fn the_contract_step_refuses_a_vocab_declaration_with_the_desugar_pass_own_diagnostic()
    -> Result<(), Box<dyn std::error::Error>> {
        // Body IR is owed a desugared program. This is what "owed" means concretely: a vocab
        // declaration whose library is unavailable stops here, with the resolution failure the desugar pass already
        // reports, rather than traveling on to become a lowering refusal at the same span.
        let program = incan_frontend::ast::Program {
            declarations: vec![undesugared_vocab_declaration()],
            ..Default::default()
        };

        let errors = body_ir::apply_body_ir_input_contract(program, Path::new("/fixture/main.incn"))
            .err()
            .ok_or("an undesugared vocab declaration must not pass the Body IR input contract")?;
        let messages = errors
            .iter()
            .map(|error| error.message.clone())
            .collect::<Vec<_>>()
            .join("; ");
        assert!(
            messages.contains("desugarer resolution failed") && messages.contains("demo.query"),
            "the desugar pass must own this diagnostic, naming the unavailable dependency: {messages}"
        );
        assert!(
            !messages.contains("input-contract violation"),
            "one unavailable-manifest condition must not produce two divergent diagnostics: {messages}"
        );
        Ok(())
    }

    #[test]
    fn the_contract_step_projects_a_body_behind_an_inactive_feature_out_of_the_program()
    -> Result<(), Box<dyn std::error::Error>> {
        // Feature projection has to happen before the module-profile gate and before lowering, so this checks the
        // program the rest of the pipeline actually receives rather than only the eventual build outcome.
        let source =
            "when feature(\"beta\"):\n    def gated() -> int:\n        return 7\n\ndef main() -> int:\n    return 1\n";
        let tokens = lexer::lex(source).map_err(|errors| CliError::failure(format!("{errors:?}")))?;
        let parsed = parser::parse(&tokens).map_err(|errors| CliError::failure(format!("{errors:?}")))?;
        assert_eq!(
            parsed.declarations.len(),
            2,
            "the fixture must carry a gated declaration, or the assertion below proves nothing"
        );

        let projected = body_ir::apply_body_ir_input_contract(parsed, Path::new("/fixture/main.incn"))
            .map_err(|errors| CliError::failure(format!("{errors:?}")))?;
        let names = projected
            .declarations
            .iter()
            .filter_map(|declaration| match &declaration.node {
                Declaration::Function(function) => Some(function.name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            ["main"],
            "a declaration behind an inactive feature must not survive the contract step"
        );
        Ok(())
    }
}
