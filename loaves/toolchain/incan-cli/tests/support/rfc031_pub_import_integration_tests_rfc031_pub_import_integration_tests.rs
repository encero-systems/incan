use super::*;
use incan_frontend::library_manifest::{LibraryManifest, ModelExport};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn run_timed_incan_command(
    label: &str,
    mut command: std::process::Command,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let timing = super::support::command_timing_started();
    let output = command.output()?;
    super::support::report_command_timing(label, timing);
    Ok(output)
}

/// Run one normal build with its existing JSON report retained only for opt-in timing attribution.
fn run_profiled_build_command(
    label: &str,
    mut command: std::process::Command,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    command.args(["--report", "json"]);
    let output = run_timed_incan_command(label, command)?;
    super::support::report_build_phase_timing(label, &output);
    Ok(output)
}

/// Ensure the normal-library report retains the internal preparation boundaries used by Oven performance work.
fn assert_library_build_phase_keys(
    output: &std::process::Output,
    expected: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let timings = report
        .get("timings_ms")
        .and_then(serde_json::Value::as_object)
        .ok_or("expected a library JSON timing report")?;
    for phase in expected {
        assert!(
            timings.contains_key(*phase),
            "expected library build timing report to contain `{phase}`: {report}"
        );
    }
    Ok(())
}

fn write_project_files(
    root: &Path,
    manifest_content: &str,
    main_source: &str,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(root.join("loaf.toml"), manifest_content)?;
    let main_path = root.join("src").join("main.incn");
    std::fs::write(&main_path, main_source)?;
    Ok(main_path)
}

/// Materialize one file from an integration fixture with its parent directories.
fn write_fixture_file(root: &Path, relative_path: &str, contents: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = root.join(relative_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    Ok(())
}

fn run_check(main_path: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut command = super::incan_command();
    command.arg("--check").arg(main_path);
    run_timed_incan_command("incan --check", command)
}

/// Check a consumer against this checkout's SDK rather than an ambient developer installation.
fn run_check_against_checkout_sdk(
    main_path: &Path,
    generated_cargo_target: &Path,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let checkout = support::repo_root();
    Ok(super::incan_command()
        .env("INCAN_SOURCE_ROOT", &checkout)
        .env("INCAN_STDLIB", checkout.join("loaves/stdlib"))
        .env_remove("INCAN_STDLIB_DIR")
        .env("INCAN_GENERATED_CARGO_TARGET_DIR", generated_cargo_target)
        // Lock-workspace preheat is covered by its own integration suite. This C-binding consumer proof needs
        // the generated program path, and its temporary project otherwise exposes macOS's `/tmp` alias as a
        // duplicate Cargo package identity.
        .env("INCAN_LOCK_PREHEAT", "0")
        .arg("check")
        .arg(main_path)
        .output()?)
}

fn run_build(main_path: &Path, out_dir: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut command = super::incan_command();
    command
        .args([
            "build",
            main_path.to_string_lossy().as_ref(),
            out_dir.to_string_lossy().as_ref(),
        ])
        .env("CARGO_NET_OFFLINE", "true");
    run_timed_incan_command("incan build", command)
}

/// Write the shared Rust dependency used by receiver-generic application and compiled-provider acceptance tests.
fn write_receiver_factory_dependency(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let rust_dir = root.join("receiver_factory");
    std::fs::create_dir_all(rust_dir.join("src"))?;
    std::fs::write(
        rust_dir.join("Cargo.toml"),
        "[package]\nname = \"receiver_factory\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    std::fs::write(
        rust_dir.join("src/lib.rs"),
        r#"pub struct Factory<T> {
    marker: std::marker::PhantomData<T>,
}

pub struct ConstructionError;

pub enum Mode {
    Input,
}

pub struct Device;

pub fn device() -> Device {
    Device
}

pub trait DeviceTrait {
    fn build_output_stream<T, D, E>(&self, value: T, data_callback: D, error_callback: E)
    where
        T: Copy,
        D: FnMut(T),
        E: FnMut(T);

    fn run_callbacks<T, D, E>(&self, data_callback: D, error_callback: E)
    where
        T: Copy + Default,
        D: FnMut(&mut [T], &OutputCallbackInfo) + Send + 'static,
        E: FnMut(String);
}

impl DeviceTrait for Device {
    fn build_output_stream<T, D, E>(&self, value: T, mut data_callback: D, mut error_callback: E)
    where
        T: Copy,
        D: FnMut(T),
        E: FnMut(T),
    {
        data_callback(value);
        error_callback(value);
    }

    fn run_callbacks<T, D, E>(&self, mut data_callback: D, mut error_callback: E)
    where
        T: Copy + Default,
        D: FnMut(&mut [T], &OutputCallbackInfo) + Send + 'static,
        E: FnMut(String),
    {
        let mut data = [T::default(); 2];
        let info = OutputCallbackInfo;
        data_callback(&mut data, &info);
        error_callback("synthetic callback error".to_string());
    }
}

pub struct OutputCallbackInfo;

pub struct PairFactory<T, U> {
    value: T,
    marker: U,
}

impl<T> Factory<T> {
    pub fn new(_size: i64, _mode: Mode) -> Result<Self, ConstructionError> {
        Ok(Self {
            marker: std::marker::PhantomData,
        })
    }
}

impl<T, U> PairFactory<T, U> {
    pub fn new(value: T, marker: U) -> Self {
        Self { value, marker }
    }

    pub fn first(value: T) -> T {
        value
    }
}
"#,
    )?;
    Ok(())
}

fn run_lock(entry_path: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut command = super::incan_command();
    command
        .args(["lock", entry_path.to_string_lossy().as_ref()])
        .env("CARGO_NET_OFFLINE", "true");
    run_timed_incan_command("incan lock", command)
}

fn run_test(target: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut command = super::incan_command();
    command
        .args(["test", target.to_string_lossy().as_ref()])
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_TEST_SHARED_TARGET_DIR", shared_test_runner_target_dir());
    run_timed_incan_command("incan test", command)
}

fn run_fmt(target: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    Ok(super::incan_command()
        .args(["fmt", target.to_string_lossy().as_ref()])
        .output()?)
}

fn run_fmt_check(target: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    Ok(super::incan_command()
        .args(["fmt", "--check", target.to_string_lossy().as_ref()])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?)
}

fn shared_test_runner_target_dir() -> PathBuf {
    support::repo_root().join("target").join("incan_e2e_shared_target")
}

fn test_runner_batch_manifest_path(project_root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let harness_root = project_root.join("target/incan_tests");
    let manifests = std::fs::read_dir(&harness_root)
        .map_err(|err| {
            format!(
                "failed reading generated test harness root {}: {err}",
                harness_root.display()
            )
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("Cargo.toml"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    match manifests.as_slice() {
        [manifest] => Ok(manifest.clone()),
        _ => Err(format!(
            "expected exactly one generated test manifest below {}, found {}",
            harness_root.display(),
            manifests.len()
        )
        .into()),
    }
}

fn run_build_lib(project_root: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut command = super::incan_command();
    command
        .args(["build", "--lib"])
        .current_dir(project_root)
        .env("CARGO_NET_OFFLINE", "true");
    run_timed_incan_command("incan build --lib", command)
}

/// Publish one project's completed output and sealed dependency collection.
///
/// Normal `build`, `run`, and `test` remain consumers. They cannot create this package handoff implicitly,
/// including when the project itself depends on a separately baked public provider.
fn bake_project(project_root: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    bake_project_with_package_features(project_root, &[])
}

/// Write an unrelated Rust provider whose generic data wrapper admits either owned handles or mutable references.
fn write_foreign_reference_provider(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let provider_root = root.join("foreign_reference_provider");
    std::fs::create_dir_all(provider_root.join("src"))?;
    std::fs::write(
        provider_root.join("Cargo.toml"),
        "[package]\nname = \"foreign_reference_provider\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    std::fs::write(
        provider_root.join("src/lib.rs"),
        r#"use core::marker::PhantomData;

pub trait QueryData {}
pub trait Component {}

pub struct FooBar<T: QueryData>(PhantomData<T>);

impl<T: QueryData> FooBar<T> {
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

pub struct Widget;
pub struct Gadget;
pub struct Entity;

impl Component for Widget {}
impl Component for Gadget {}
impl<T: Component> QueryData for &mut T {}
impl<A: QueryData, B: QueryData> QueryData for (A, B) {}
impl QueryData for Entity {}
"#,
    )?;
    Ok(())
}

/// Write a provider whose derive macro deliberately differs between the compiler probe and a field-bearing type.
fn write_shape_sensitive_reference_provider(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let derive_root = root.join("shape_sensitive_derive");
    std::fs::create_dir_all(derive_root.join("src"))?;
    std::fs::write(
        derive_root.join("Cargo.toml"),
        "[package]\nname = \"shape_sensitive_derive\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nproc-macro = true\n",
    )?;
    std::fs::write(
        derive_root.join("src/lib.rs"),
        r#"use proc_macro::TokenStream;

#[proc_macro_derive(Component)]
pub fn component(input: TokenStream) -> TokenStream {
    let source = input.to_string();
    let name = source
        .split_whitespace()
        .nth(1)
        .expect("derive input should contain a type name")
        .trim_end_matches(';');
    if !name.starts_with("__IncanDeriveProbe") {
        return TokenStream::new();
    }
    format!("impl shape_sensitive_provider::Component for {name} {{}}")
        .parse()
        .expect("generated Component implementation should parse")
}
"#,
    )?;

    let provider_root = root.join("shape_sensitive_provider");
    std::fs::create_dir_all(provider_root.join("src"))?;
    std::fs::write(
        provider_root.join("Cargo.toml"),
        "[package]\nname = \"shape_sensitive_provider\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nshape_sensitive_derive = { path = \"../shape_sensitive_derive\" }\n",
    )?;
    std::fs::write(
        provider_root.join("src/lib.rs"),
        r#"use core::marker::PhantomData;

pub use shape_sensitive_derive::Component;

pub trait Component {}
pub trait QueryData {}

pub struct FooBar<T: QueryData>(PhantomData<T>);

impl<T: QueryData> FooBar<T> {
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T: Component> QueryData for &mut T {}
"#,
    )?;
    Ok(())
}

/// Publish one project for an explicit package-feature selection.
fn bake_project_with_package_features(
    project_root: &Path,
    package_feature_args: &[&str],
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut command = super::incan_command();
    command
        .args(["oven", "bake", "--project", "."])
        .args(package_feature_args)
        .current_dir(project_root)
        .env("CARGO_NET_OFFLINE", "true");
    support::configure_explicit_oven_bake_command(&mut command)?;
    run_timed_incan_command("incan oven bake --project", command)
}

/// Publish a public-library provider before a separate consumer imports its package Loaf.
fn bake_library_provider(project_root: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    bake_project(project_root)
}

/// Publish one source-backed provider whose checked vocabulary metadata and desugarer are part of the bake.
fn write_and_bake_source_vocab_fixture_provider(
    root: &Path,
    dependency_key: &str,
    project_name: &str,
    source: &str,
    companion_package: &str,
    companion_source: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let provider_root = root.join("deps").join(dependency_key);
    std::fs::create_dir_all(provider_root.join("src"))?;
    std::fs::write(
        provider_root.join("loaf.toml"),
        format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n"),
    )?;
    std::fs::write(provider_root.join("src/lib.incn"), source)?;
    write_vocab_companion_crate_with_source(&provider_root, "vocab_companion", companion_package, companion_source)?;
    let provider_bake = bake_library_provider(&provider_root)?;
    assert!(
        provider_bake.status.success(),
        "expected source-backed {dependency_key} vocabulary fixture provider to bake.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&provider_bake.stdout),
        String::from_utf8_lossy(&provider_bake.stderr)
    );
    Ok(())
}

fn filterkit_vocab_companion_source() -> &'static str {
    r#"use incan_vocab::{DesugarError, DesugarOutput, HelperBinding, IncanExpr, KeywordActivation, KeywordPlacement, KeywordRegistration, KeywordSpec, KeywordSurfaceKind, LibraryManifest, VocabDesugarer, VocabRegistration, VocabSyntaxNode};

#[derive(Default)]
struct FilterkitDesugarer;

impl VocabDesugarer for FilterkitDesugarer {
    fn desugar(&self, _node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        Ok(DesugarOutput::Expression(IncanExpr::Call {
            callee: Box::new(IncanExpr::Helper("filter".to_string())),
            args: vec![IncanExpr::Int(1)],
        }))
    }
}

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_keyword_registration(KeywordRegistration {
            activation: KeywordActivation::OnImport { namespace: "filterkit.dsl".to_string() },
            keywords: vec![KeywordSpec {
                name: "where".to_string(),
                surface_kind: KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        })
        .with_library_manifest(LibraryManifest {
            helper_bindings: vec![HelperBinding { key: "filter".to_string(), exported_name: "filter".to_string() }],
            ..LibraryManifest::default()
        })
        .with_desugarer(FilterkitDesugarer)
}

incan_vocab::export_wasm_desugarer!(FilterkitDesugarer);
"#
}

fn helperkit_vocab_companion_source() -> &'static str {
    r#"use incan_vocab::{DesugarError, DesugarOutput, HelperBinding, IncanExpr, KeywordActivation, KeywordPlacement, KeywordRegistration, KeywordSpec, KeywordSurfaceKind, LibraryManifest, VocabDesugarer, VocabRegistration, VocabSyntaxNode};

#[derive(Default)]
struct HelperkitDesugarer;

impl VocabDesugarer for HelperkitDesugarer {
    fn desugar(&self, _node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        Ok(DesugarOutput::Expression(IncanExpr::Call {
            callee: Box::new(IncanExpr::Helper("aggregate_as".to_string())),
            args: vec![
                IncanExpr::Call {
                    callee: Box::new(IncanExpr::Helper("lit".to_string())),
                    args: vec![IncanExpr::Int(5)],
                },
                IncanExpr::Str("total".to_string()),
            ],
        }))
    }
}

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_keyword_registration(KeywordRegistration {
            activation: KeywordActivation::OnImport { namespace: "helperkit.dsl".to_string() },
            keywords: vec![KeywordSpec {
                name: "where".to_string(),
                surface_kind: KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        })
        .with_library_manifest(LibraryManifest {
            helper_bindings: vec![
                HelperBinding { key: "lit".to_string(), exported_name: "lit".to_string() },
                HelperBinding { key: "aggregate_as".to_string(), exported_name: "aggregate_as".to_string() },
            ],
            ..LibraryManifest::default()
        })
        .with_desugarer(HelperkitDesugarer)
}

incan_vocab::export_wasm_desugarer!(HelperkitDesugarer);
"#
}

fn quality_vocab_companion_source() -> &'static str {
    r#"use incan_vocab::{ClauseSurface, DeclarationSurface, DesugarError, DesugarOutput, DslSurface, IncanExpr, KeywordActivation, KeywordRegistration, KeywordSpec, LibraryManifest, ScopedSurfaceDescriptor, ScopedSurfaceReceiver, VocabBodyItem, VocabDesugarer, VocabRegistration, VocabSyntaxNode};

#[derive(Default)]
struct QualityDesugarer;

impl VocabDesugarer for QualityDesugarer {
    fn desugar(&self, node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        let VocabSyntaxNode::Declaration(declaration) = node else {
            return Err(DesugarError::new("quality expects a declaration"));
        };
        let clauses = declaration
            .body
            .iter()
            .filter_map(|item| match item {
                VocabBodyItem::Clause(clause) => Some(clause),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (keyword, compound_tokens) in [
            ("FROM", Vec::<String>::new()),
            ("REQUIRE", Vec::new()),
            ("GROUP", vec!["BY".to_string()]),
            ("EXPECT", Vec::new()),
        ] {
            if !clauses
                .iter()
                .any(|clause| clause.keyword == keyword && clause.compound_tokens == compound_tokens)
            {
                return Err(DesugarError::new(format!("quality request is missing {keyword}")));
            }
        }
        Ok(DesugarOutput::Expression(IncanExpr::Int(7)))
    }
}

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_keyword_registration(KeywordRegistration {
            activation: KeywordActivation::OnImport { namespace: "querykit.query".to_string() },
            keywords: vec![
                KeywordSpec::block("quality"),
                KeywordSpec::block("FROM").in_block("quality"),
                KeywordSpec::block("REQUIRE").in_block("quality"),
                KeywordSpec::block("GROUP").with_compound_tokens(["BY"]).in_block("quality"),
                KeywordSpec::block("EXPECT").in_block("quality"),
            ],
            valid_decorators: Vec::new(),
        })
        .with_surface(
            DslSurface::on_import("querykit.query")
                .with_declaration(
                    DeclarationSurface::named("quality")
                        .with_mixed_body()
                        .desugars_to_expression()
                        .with_clauses([
                            ClauseSurface::expr("FROM").optional(),
                            ClauseSurface::expr_list("GROUP BY").repeating().after("FROM"),
                            ClauseSurface::expr_list("EXPECT").repeating().after("FROM"),
                            ClauseSurface::expr_list("REQUIRE").repeating().after("FROM"),
                        ]),
                )
                .with_scoped_surface(
                    ScopedSurfaceDescriptor::leading_dot_path("quality.group.field")
                        .in_clause_body("quality", "GROUP")
                        .with_receiver(ScopedSurfaceReceiver::clause("FROM")),
                ),
        )
        .with_library_manifest(LibraryManifest::default())
        .with_desugarer(QualityDesugarer)
}

incan_vocab::export_wasm_desugarer!(QualityDesugarer);
"#
}

fn querykit_expression_clause_vocab_companion_source() -> &'static str {
    r#"use incan_vocab::{ClauseSurface, DeclarationSurface, DesugarError, DesugarOutput, DslSurface, IncanExpr, VocabBodyItem, VocabDesugarer, VocabRegistration, VocabSyntaxNode};

#[derive(Default)]
struct QuerykitExpressionClauseDesugarer;

impl VocabDesugarer for QuerykitExpressionClauseDesugarer {
    fn desugar(&self, node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        let VocabSyntaxNode::Declaration(declaration) = node else {
            return Err(DesugarError::new("query expects a declaration"));
        };
        if !declaration.body.iter().any(|item| {
            matches!(item, VocabBodyItem::Clause(clause) if clause.keyword == "SELECT")
        }) {
            return Err(DesugarError::new("missing SELECT clause payload"));
        }
        Ok(DesugarOutput::Expression(IncanExpr::Int(7)))
    }
}

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_surface(
            DslSurface::on_import("querykit.query").with_declaration(
                DeclarationSurface::named("query")
                    .with_clause_body()
                    .desugars_to_expression()
                    .with_clauses([
                        ClauseSurface::expr("FROM").required(),
                        ClauseSurface::expr_list("GROUP BY").optional(),
                        ClauseSurface::expr_list("SELECT").required(),
                        ClauseSurface::expr_list("ORDER BY").optional(),
                        ClauseSurface::nested_items("WINDOW BY").optional(),
                    ]),
            ),
        )
        .with_desugarer(QuerykitExpressionClauseDesugarer)
}

incan_vocab::export_wasm_desugarer!(QuerykitExpressionClauseDesugarer);
"#
}

fn querykit_helper_vocab_companion_source() -> &'static str {
    r#"use incan_vocab::{DesugarError, DesugarOutput, HelperBinding, IncanExpr, KeywordActivation, KeywordPlacement, KeywordRegistration, KeywordSpec, KeywordSurfaceKind, LibraryManifest, VocabDesugarer, VocabRegistration, VocabSyntaxNode};

#[derive(Default)]
struct QuerykitHelperDesugarer;

impl VocabDesugarer for QuerykitHelperDesugarer {
    fn desugar(&self, _node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        Ok(DesugarOutput::Expression(IncanExpr::Tuple(vec![
            IncanExpr::Call {
                callee: Box::new(IncanExpr::Helper("aggregate_as".to_string())),
                args: vec![
                    IncanExpr::Call {
                        callee: Box::new(IncanExpr::Helper("lit".to_string())),
                        args: vec![IncanExpr::Int(5)],
                    },
                    IncanExpr::Str("adjusted".to_string()),
                ],
            },
            IncanExpr::Call {
                callee: Box::new(IncanExpr::Helper("aggregate_as".to_string())),
                args: vec![
                    IncanExpr::Call {
                        callee: Box::new(IncanExpr::Helper("count".to_string())),
                        args: Vec::new(),
                    },
                    IncanExpr::Str("order_count".to_string()),
                ],
            },
        ])))
    }
}

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_keyword_registration(KeywordRegistration {
            activation: KeywordActivation::OnImport { namespace: "querykit.dsl".to_string() },
            keywords: vec![KeywordSpec {
                name: "where".to_string(),
                surface_kind: KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        })
        .with_library_manifest(LibraryManifest {
            helper_bindings: vec![
                HelperBinding { key: "lit".to_string(), exported_name: "lit".to_string() },
                HelperBinding { key: "count".to_string(), exported_name: "count".to_string() },
                HelperBinding { key: "aggregate_as".to_string(), exported_name: "aggregate_as".to_string() },
            ],
            ..LibraryManifest::default()
        })
        .with_desugarer(QuerykitHelperDesugarer)
}

incan_vocab::export_wasm_desugarer!(QuerykitHelperDesugarer);
"#
}

/// Run one single-library build with the machine-readable phase report used by Oven timing checks.
fn run_profiled_build_lib(
    label: &str,
    project_root: &Path,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut command = super::incan_command();
    command
        .args(["build", "--lib"])
        .current_dir(project_root)
        .env("CARGO_NET_OFFLINE", "true");
    run_profiled_build_command(label, command)
}

/// Run a normal Oven route with a failing Cargo binary first on PATH.
///
/// This is a behavioral boundary: a successful command proves that its completed-Loaf materialization used only
/// the selected direct-rustc closure rather than merely avoiding Cargo in an outer command.
#[cfg(unix)]
fn run_incan_with_failing_cargo_guard(
    project_root: &Path,
    guard_root: &Path,
    cargo_marker: &Path,
    args: &[&str],
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(guard_root)?;
    let cargo_guard = guard_root.join("cargo");
    std::fs::write(
        &cargo_guard,
        format!("#!/bin/sh\nprintf cargo > \"{}\"\nexit 97\n", cargo_marker.display()),
    )?;
    let mut permissions = std::fs::metadata(&cargo_guard)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&cargo_guard, permissions)?;
    let mut paths = vec![guard_root.to_path_buf()];
    if let Some(inherited) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&inherited));
    }
    Ok(super::incan_command()
        .args(args)
        .current_dir(project_root)
        .env("CARGO_NET_OFFLINE", "true")
        .env("PATH", std::env::join_paths(paths)?)
        .output()?)
}

fn run_build_lib_artifact_only(project_root: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut command = super::incan_command();
    command
        .args(["build", "--lib"])
        .current_dir(project_root)
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_INTERNAL_LIBRARY_ARTIFACT_ONLY", "1");
    run_timed_incan_command("incan build --lib (artifact only)", command)
}

fn write_pub_boundary_type_fidelity_library(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let producer_root = root.join("pub_boundary_library");
    std::fs::create_dir_all(producer_root.join("src"))?;
    std::fs::write(
        producer_root.join("loaf.toml"),
        "[project]\nname = \"pub_boundary_core\"\nversion = \"0.1.0\"\n",
    )?;
    std::fs::write(
        producer_root.join("src/dataset.incn"),
        r#"pub model SessionError:
  pub kind: str

pub trait DataSet[T]:
  def to_substrait_plan(self) -> int: ...

pub trait BoundedDataSet[T] with DataSet[T]:
  pass

@derive(Clone)
pub class DataFrame[T] with BoundedDataSet:
  pub _type_witness: list[T]

  def to_substrait_plan(self) -> int:
    return 1

@derive(Clone)
pub class LazyFrame[T] with BoundedDataSet:
  pub _type_witness: list[T]

  def to_substrait_plan(self) -> int:
    return 1

  def collect(self) -> Result[DataFrame[T], SessionError]:
    return Ok(DataFrame[T](_type_witness=[]))
"#,
    )?;
    std::fs::write(
        producer_root.join("src/functions.incn"),
        r#"from dataset import DataSet

pub def display[T](data: DataSet[T]) -> None:
  print(data.to_substrait_plan())
"#,
    )?;
    std::fs::write(
        producer_root.join("src/lib.incn"),
        "pub from dataset import SessionError, DataSet, BoundedDataSet, DataFrame, LazyFrame\npub from functions import display\n",
    )?;

    let producer_build = bake_library_provider(&producer_root)?;
    assert!(
        producer_build.status.success(),
        "expected pub-boundary library build to succeed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&producer_build.stdout),
        String::from_utf8_lossy(&producer_build.stderr)
    );
    Ok(())
}

fn write_minimal_library_crate(artifact_root: &Path, package_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(artifact_root.join("src"))?;
    std::fs::write(
        artifact_root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n"
        ),
    )?;
    std::fs::write(artifact_root.join("src/lib.rs"), "pub fn linked() {}\n")?;
    Ok(())
}

fn write_vocab_companion_crate(
    project_root: &Path,
    relative_path: &str,
    package_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let crate_root = project_root.join(relative_path);
    std::fs::create_dir_all(crate_root.join("src"))?;
    std::fs::write(
        crate_root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nincan_vocab = {{ path = \"{}\" }}\n\n[lib]\npath = \"src/lib.rs\"\n",
            support::repo_root()
                .join(oven_model::toolchain_layout::development_support_crate_dir(
                    "incan_vocab"
                ))
                .display()
        ),
    )?;
    std::fs::write(
        crate_root.join("src/lib.rs"),
        "pub fn library_vocab() -> incan_vocab::VocabRegistration {\n    incan_vocab::VocabRegistration::new().with_keyword_registration(\n        incan_vocab::KeywordRegistration {\n            activation: incan_vocab::KeywordActivation::OnImport {\n                namespace: \"widgets.dsl\".to_string(),\n            },\n            keywords: vec![incan_vocab::KeywordSpec::new(\n                \"await\",\n                incan_vocab::KeywordSurfaceKind::ControlFlow,\n            )],\n            valid_decorators: vec![\"route\".to_string()],\n        },\n    )\n}\n",
    )?;
    Ok(())
}

fn write_vocab_companion_crate_with_assert_keyword(
    project_root: &Path,
    relative_path: &str,
    package_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let crate_root = project_root.join(relative_path);
    std::fs::create_dir_all(crate_root.join("src"))?;
    std::fs::write(
        crate_root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nincan_vocab = {{ path = \"{}\" }}\n\n[lib]\npath = \"src/lib.rs\"\n",
            support::repo_root()
                .join(oven_model::toolchain_layout::development_support_crate_dir(
                    "incan_vocab"
                ))
                .display()
        ),
    )?;
    std::fs::write(
        crate_root.join("src/lib.rs"),
        "pub fn library_vocab() -> incan_vocab::VocabRegistration {\n    incan_vocab::VocabRegistration::new().with_keyword_registration(\n        incan_vocab::KeywordRegistration {\n            activation: incan_vocab::KeywordActivation::OnImport {\n                namespace: \"widgets.dsl\".to_string(),\n            },\n            keywords: vec![incan_vocab::KeywordSpec::new(\n                \"assert\",\n                incan_vocab::KeywordSurfaceKind::ControlFlow,\n            )],\n            valid_decorators: vec![\"route\".to_string()],\n        },\n    )\n}\n",
    )?;
    Ok(())
}

fn write_vocab_companion_crate_with_source(
    project_root: &Path,
    relative_path: &str,
    package_name: &str,
    lib_source: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let crate_root = project_root.join(relative_path);
    std::fs::create_dir_all(crate_root.join("src"))?;
    std::fs::write(
        crate_root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nincan_vocab = {{ path = \"{}\" }}\n\n[lib]\npath = \"src/lib.rs\"\ncrate-type = [\"rlib\", \"cdylib\"]\n",
            support::repo_root()
                .join(oven_model::toolchain_layout::development_support_crate_dir(
                    "incan_vocab"
                ))
                .display()
        ),
    )?;
    std::fs::write(crate_root.join("src/lib.rs"), lib_source)?;
    Ok(())
}

fn wat_bytes_string(bytes: &[u8]) -> String {
    let mut escaped = String::new();
    for byte in bytes {
        escaped.push('\\');
        escaped.push_str(&format!("{byte:02x}"));
    }
    escaped
}

fn wat_data_string(text: &str) -> String {
    wat_bytes_string(text.as_bytes())
}

fn wat_i32_cell(value: i32) -> String {
    wat_bytes_string(&value.to_le_bytes())
}

fn compile_desugarer_wasm(
    status_code: i32,
    output_payload: &str,
    error_payload: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let output_ptr_cell = 0usize;
    let output_len_cell = 4usize;
    let error_ptr_cell = 8usize;
    let error_len_cell = 12usize;
    let input_ptr_cell = 16usize;
    let input_capacity_cell = 20usize;
    let input_len_cell = 24usize;
    let output_offset = 128usize;
    let output_len = output_payload.len();
    let error_offset = output_offset + output_len + 32;
    let input_offset = error_offset + error_payload.len() + 32;
    let input_capacity = 4096usize;
    let wat_source = format!(
        r#"(module
  (memory (export "memory") 1)
  (global $input_ptr_cell (export "__incan_input_ptr") i32 (i32.const {input_ptr_cell}))
  (global (export "__incan_input_capacity") i32 (i32.const {input_capacity_cell}))
  (global $input_len_cell (export "__incan_input_len") i32 (i32.const {input_len_cell}))
  (global (export "__incan_output_ptr") i32 (i32.const {output_ptr_cell}))
  (global (export "__incan_output_len") i32 (i32.const {output_len_cell}))
  (global (export "__incan_error_ptr") i32 (i32.const {error_ptr_cell}))
  (global (export "__incan_error_len") i32 (i32.const {error_len_cell}))
  (data (i32.const {output_ptr_cell}) "{output_ptr_data}")
  (data (i32.const {output_len_cell}) "{output_len_data}")
  (data (i32.const {error_ptr_cell}) "{error_ptr_data}")
  (data (i32.const {error_len_cell}) "{error_len_data}")
  (data (i32.const {input_ptr_cell}) "{input_ptr_data}")
  (data (i32.const {input_capacity_cell}) "{input_capacity_data}")
  (data (i32.const {input_len_cell}) "{input_len_data}")
  (data (i32.const {output_offset}) "{output_data}")
  (data (i32.const {error_offset}) "{error_data}")
  (func (export "__incan_init_desugarer"))
  (func (export "desugar_block") (result i32)
    (i32.const {status_code})
  )
)"#,
        output_ptr_cell = output_ptr_cell,
        output_len_cell = output_len_cell,
        error_ptr_cell = error_ptr_cell,
        error_len_cell = error_len_cell,
        input_ptr_cell = input_ptr_cell,
        input_capacity_cell = input_capacity_cell,
        input_len_cell = input_len_cell,
        output_ptr_data = wat_i32_cell(output_offset as i32),
        output_len_data = wat_i32_cell(output_payload.len() as i32),
        error_ptr_data = wat_i32_cell(error_offset as i32),
        error_len_data = wat_i32_cell(error_payload.len() as i32),
        input_ptr_data = wat_i32_cell(input_offset as i32),
        input_capacity_data = wat_i32_cell(input_capacity as i32),
        input_len_data = wat_i32_cell(0),
        output_data = wat_data_string(output_payload),
        error_data = wat_data_string(error_payload),
    );
    Ok(wat::parse_str(wat_source)?)
}

fn compile_desugarer_wasm_requiring_request(
    output_payload: &str,
    error_payload: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let output_ptr_cell = 0usize;
    let output_len_cell = 4usize;
    let error_ptr_cell = 8usize;
    let error_len_cell = 12usize;
    let input_ptr_cell = 16usize;
    let input_capacity_cell = 20usize;
    let input_len_cell = 24usize;
    let output_offset = 128usize;
    let output_len = output_payload.len();
    let error_offset = output_offset + output_len + 32;
    let input_offset = error_offset + error_payload.len() + 32;
    let input_capacity = 4096usize;
    let wat_source = format!(
        r#"(module
  (memory (export "memory") 1)
  (global $input_ptr_cell (export "__incan_input_ptr") i32 (i32.const {input_ptr_cell}))
  (global (export "__incan_input_capacity") i32 (i32.const {input_capacity_cell}))
  (global $input_len_cell (export "__incan_input_len") i32 (i32.const {input_len_cell}))
  (global (export "__incan_output_ptr") i32 (i32.const {output_ptr_cell}))
  (global (export "__incan_output_len") i32 (i32.const {output_len_cell}))
  (global (export "__incan_error_ptr") i32 (i32.const {error_ptr_cell}))
  (global (export "__incan_error_len") i32 (i32.const {error_len_cell}))
  (data (i32.const {output_ptr_cell}) "{output_ptr_data}")
  (data (i32.const {output_len_cell}) "{output_len_data}")
  (data (i32.const {error_ptr_cell}) "{error_ptr_data}")
  (data (i32.const {error_len_cell}) "{error_len_data}")
  (data (i32.const {input_ptr_cell}) "{input_ptr_data}")
  (data (i32.const {input_capacity_cell}) "{input_capacity_data}")
  (data (i32.const {input_len_cell}) "{input_len_data}")
  (data (i32.const {output_offset}) "{output_data}")
  (data (i32.const {error_offset}) "{error_data}")
  (func (export "__incan_init_desugarer"))
  (func (export "desugar_block") (result i32)
    global.get $input_len_cell
    i32.load
    i32.eqz
    if (result i32)
      (i32.const 1)
    else
      global.get $input_ptr_cell
      i32.load
      i32.load8_u
      i32.const 123
      i32.eq
      if (result i32)
        (i32.const 0)
      else
        (i32.const 1)
      end
    end
  )
)"#,
        output_ptr_cell = output_ptr_cell,
        output_len_cell = output_len_cell,
        error_ptr_cell = error_ptr_cell,
        error_len_cell = error_len_cell,
        input_ptr_cell = input_ptr_cell,
        input_capacity_cell = input_capacity_cell,
        input_len_cell = input_len_cell,
        output_ptr_data = wat_i32_cell(output_offset as i32),
        output_len_data = wat_i32_cell(output_payload.len() as i32),
        error_ptr_data = wat_i32_cell(error_offset as i32),
        error_len_data = wat_i32_cell(error_payload.len() as i32),
        input_ptr_data = wat_i32_cell(input_offset as i32),
        input_capacity_data = wat_i32_cell(input_capacity as i32),
        input_len_data = wat_i32_cell(0),
        output_data = wat_data_string(output_payload),
        error_data = wat_data_string(error_payload),
    );
    Ok(wat::parse_str(wat_source)?)
}

fn compile_desugarer_wasm_requiring_request_substring(
    output_payload: &str,
    error_payload: &str,
    needle: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let output_ptr_cell = 0usize;
    let output_len_cell = 4usize;
    let error_ptr_cell = 8usize;
    let error_len_cell = 12usize;
    let input_ptr_cell = 16usize;
    let input_capacity_cell = 20usize;
    let input_len_cell = 24usize;
    let output_offset = 128usize;
    let output_len = output_payload.len();
    let error_offset = output_offset + output_len + 32;
    let input_offset = error_offset + error_payload.len() + 32;
    let input_capacity = 16_384usize;
    let needle_offset = input_offset + input_capacity + 32;
    let needle_len = needle.len();
    let wat_source = format!(
        r#"(module
  (memory (export "memory") 1)
  (global $input_ptr_cell (export "__incan_input_ptr") i32 (i32.const {input_ptr_cell}))
  (global (export "__incan_input_capacity") i32 (i32.const {input_capacity_cell}))
  (global $input_len_cell (export "__incan_input_len") i32 (i32.const {input_len_cell}))
  (global (export "__incan_output_ptr") i32 (i32.const {output_ptr_cell}))
  (global (export "__incan_output_len") i32 (i32.const {output_len_cell}))
  (global (export "__incan_error_ptr") i32 (i32.const {error_ptr_cell}))
  (global (export "__incan_error_len") i32 (i32.const {error_len_cell}))
  (data (i32.const {output_ptr_cell}) "{output_ptr_data}")
  (data (i32.const {output_len_cell}) "{output_len_data}")
  (data (i32.const {error_ptr_cell}) "{error_ptr_data}")
  (data (i32.const {error_len_cell}) "{error_len_data}")
  (data (i32.const {input_ptr_cell}) "{input_ptr_data}")
  (data (i32.const {input_capacity_cell}) "{input_capacity_data}")
  (data (i32.const {input_len_cell}) "{input_len_data}")
  (data (i32.const {output_offset}) "{output_data}")
  (data (i32.const {error_offset}) "{error_data}")
  (data (i32.const {needle_offset}) "{needle_data}")
  (func (export "__incan_init_desugarer"))
  (func $matches_at (param $pos i32) (result i32)
    (local $j i32)
    (block $fail
      (loop $scan
        local.get $j
        i32.const {needle_len}
        i32.ge_u
        if
          i32.const 1
          return
        end
        local.get $pos
        local.get $j
        i32.add
        i32.load8_u
        i32.const {needle_offset}
        local.get $j
        i32.add
        i32.load8_u
        i32.ne
        br_if $fail
        local.get $j
        i32.const 1
        i32.add
        local.set $j
        br $scan
      )
    )
    i32.const 0
  )
  (func (export "desugar_block") (result i32)
    (local $input_ptr i32)
    (local $input_len i32)
    (local $end i32)
    (local $i i32)
    global.get $input_ptr_cell
    i32.load
    local.set $input_ptr
    global.get $input_len_cell
    i32.load
    local.set $input_len
    local.get $input_len
    i32.const {needle_len}
    i32.lt_u
    if
      i32.const 1
      return
    end
    local.get $input_ptr
    local.get $input_len
    i32.add
    i32.const {needle_len}
    i32.sub
    i32.const 1
    i32.add
    local.set $end
    local.get $input_ptr
    local.set $i
    (block $not_found
      (loop $search
        local.get $i
        local.get $end
        i32.ge_u
        br_if $not_found
        local.get $i
        call $matches_at
        if
          i32.const 0
          return
        end
        local.get $i
        i32.const 1
        i32.add
        local.set $i
        br $search
      )
    )
    i32.const 1
  )
)"#,
        output_ptr_cell = output_ptr_cell,
        output_len_cell = output_len_cell,
        error_ptr_cell = error_ptr_cell,
        error_len_cell = error_len_cell,
        input_ptr_cell = input_ptr_cell,
        input_capacity_cell = input_capacity_cell,
        input_len_cell = input_len_cell,
        output_ptr_data = wat_i32_cell(output_offset as i32),
        output_len_data = wat_i32_cell(output_payload.len() as i32),
        error_ptr_data = wat_i32_cell(error_offset as i32),
        error_len_data = wat_i32_cell(error_payload.len() as i32),
        input_ptr_data = wat_i32_cell(input_offset as i32),
        input_capacity_data = wat_i32_cell(input_capacity as i32),
        input_len_data = wat_i32_cell(0),
        output_data = wat_data_string(output_payload),
        error_data = wat_data_string(error_payload),
        needle_offset = needle_offset,
        needle_len = needle_len,
        needle_data = wat_data_string(needle),
    );
    Ok(wat::parse_str(wat_source)?)
}

fn write_pub_library_with_vocab_desugarer(
    root: &Path,
    dependency_key: &str,
    manifest_name: &str,
    desugarer_bytes: &[u8],
    keyword: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let artifact_root = root.join("deps").join(dependency_key).join("target").join("lib");
    std::fs::create_dir_all(artifact_root.join("desugarers"))?;
    write_minimal_library_crate(&artifact_root, manifest_name)?;
    let desugarer_path = artifact_root.join("desugarers").join("routes_desugarer.wasm");
    std::fs::write(&desugarer_path, desugarer_bytes)?;

    let mut manifest = LibraryManifest::new(manifest_name, "0.1.0");
    manifest.vocab = Some(incan_frontend::library_manifest::VocabExports {
        crate_path: "vocab_companion".to_string(),
        package_name: "vocab_companion".to_string(),
        keyword_registrations: vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: format!("{dependency_key}.dsl"),
            },
            keywords: vec![incan_vocab::KeywordSpec {
                name: keyword.to_string(),
                surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: incan_vocab::KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        }],
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: Some(incan_frontend::library_manifest::VocabDesugarerArtifact {
            artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
            abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
            relative_path: "desugarers/routes_desugarer.wasm".to_string(),
            target: "wasm32-wasip1".to_string(),
            profile: "release".to_string(),
            entrypoint: "desugar_block".to_string(),
            sha256: hex::encode(Sha256::digest(desugarer_bytes)),
        }),
    });
    manifest.write_to_path(&artifact_root.join(format!("{manifest_name}.incnlib")))?;
    Ok(())
}

fn write_pub_library_with_querykit_surface_desugarer(
    root: &Path,
    desugarer_bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let artifact_root = root.join("deps").join("querykit").join("target").join("lib");
    std::fs::create_dir_all(artifact_root.join("desugarers"))?;
    write_minimal_library_crate(&artifact_root, "querykit_core")?;
    let desugarer_path = artifact_root.join("desugarers").join("querykit_desugarer.wasm");
    std::fs::write(&desugarer_path, desugarer_bytes)?;

    let mut manifest = LibraryManifest::new("querykit_core", "0.1.0");
    manifest.vocab = Some(incan_frontend::library_manifest::VocabExports {
        crate_path: "vocab_companion".to_string(),
        package_name: "vocab_companion".to_string(),
        keyword_registrations: vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "querykit.query".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec {
                name: "query".to_string(),
                surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: incan_vocab::KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        }],
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("querykit.query")
                .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.field")
                        .in_declaration_body("query")
                        .with_receiver(incan_vocab::ScopedSurfaceReceiver::OwningDeclaration),
                ),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: Some(incan_frontend::library_manifest::VocabDesugarerArtifact {
            artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
            abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
            relative_path: "desugarers/querykit_desugarer.wasm".to_string(),
            target: "wasm32-wasip1".to_string(),
            profile: "release".to_string(),
            entrypoint: "desugar_block".to_string(),
            sha256: hex::encode(Sha256::digest(desugarer_bytes)),
        }),
    });
    manifest.write_to_path(&artifact_root.join("querykit_core.incnlib"))?;
    Ok(())
}

fn write_pub_library_with_querykit_select_desugarer(
    root: &Path,
    desugarer_bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let artifact_root = root.join("deps").join("querykit").join("target").join("lib");
    std::fs::create_dir_all(artifact_root.join("desugarers"))?;
    write_minimal_library_crate(&artifact_root, "querykit_core")?;
    let desugarer_path = artifact_root.join("desugarers").join("querykit_desugarer.wasm");
    std::fs::write(&desugarer_path, desugarer_bytes)?;

    let metadata = incan_vocab::VocabRegistration::new()
        .with_surface(
            incan_vocab::DslSurface::on_import("querykit.query").with_declaration(
                incan_vocab::DeclarationSurface::named("query")
                    .with_clause_body()
                    .desugars_to_expression()
                    .with_clause(
                        incan_vocab::ClauseSurface::expr_list("SELECT")
                            .with_expression_item_modifiers([
                                incan_vocab::ExpressionItemModifierSurface::expr("for"),
                                incan_vocab::ExpressionItemModifierSurface::expr("with"),
                            ])
                            .required(),
                    ),
            ),
        )
        .metadata();
    let mut manifest = LibraryManifest::new("querykit_core", "0.1.0");
    manifest.vocab = Some(incan_frontend::library_manifest::VocabExports {
        crate_path: "vocab_companion".to_string(),
        package_name: "vocab_companion".to_string(),
        keyword_registrations: metadata.keyword_registrations,
        dsl_surfaces: metadata.dsl_surfaces,
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: Some(incan_frontend::library_manifest::VocabDesugarerArtifact {
            artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
            abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
            relative_path: "desugarers/querykit_desugarer.wasm".to_string(),
            target: "wasm32-wasip1".to_string(),
            profile: "release".to_string(),
            entrypoint: "desugar_block".to_string(),
            sha256: hex::encode(Sha256::digest(desugarer_bytes)),
        }),
    });
    manifest.write_to_path(&artifact_root.join("querykit_core.incnlib"))?;
    Ok(())
}

fn write_pub_library_with_querykit_expression_clause_desugarer(
    root: &Path,
    desugarer_bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let artifact_root = root.join("deps").join("querykit").join("target").join("lib");
    std::fs::create_dir_all(artifact_root.join("desugarers"))?;
    write_minimal_library_crate(&artifact_root, "querykit_core")?;
    let desugarer_path = artifact_root.join("desugarers").join("querykit_desugarer.wasm");
    std::fs::write(&desugarer_path, desugarer_bytes)?;

    let metadata = incan_vocab::VocabRegistration::new()
        .with_surface(
            incan_vocab::DslSurface::on_import("querykit.query").with_declaration(
                incan_vocab::DeclarationSurface::named("query")
                    .with_clause_body()
                    .desugars_to_expression()
                    .with_clauses([
                        incan_vocab::ClauseSurface::expr("FROM").required(),
                        incan_vocab::ClauseSurface::expr_list("GROUP BY").optional(),
                        incan_vocab::ClauseSurface::expr_list("SELECT").required(),
                        incan_vocab::ClauseSurface::expr_list("ORDER BY").optional(),
                        incan_vocab::ClauseSurface::nested_items("WINDOW BY").optional(),
                    ]),
            ),
        )
        .metadata();
    let mut manifest = LibraryManifest::new("querykit_core", "0.1.0");
    manifest.vocab = Some(incan_frontend::library_manifest::VocabExports {
        crate_path: "vocab_companion".to_string(),
        package_name: "vocab_companion".to_string(),
        keyword_registrations: metadata.keyword_registrations,
        dsl_surfaces: metadata.dsl_surfaces,
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: Some(incan_frontend::library_manifest::VocabDesugarerArtifact {
            artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
            abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
            relative_path: "desugarers/querykit_desugarer.wasm".to_string(),
            target: "wasm32-wasip1".to_string(),
            profile: "release".to_string(),
            entrypoint: "desugar_block".to_string(),
            sha256: hex::encode(Sha256::digest(desugarer_bytes)),
        }),
    });
    manifest.write_to_path(&artifact_root.join("querykit_core.incnlib"))?;
    Ok(())
}

fn write_source_pub_library_with_vocab_desugarer_and_query_helpers(
    root: &Path,
    with_vocab: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let producer_root = root.join("deps").join("querykit");

    // ---- Context: source-backed helper library ----
    std::fs::create_dir_all(producer_root.join("src"))?;
    std::fs::write(
        producer_root.join("loaf.toml"),
        if with_vocab {
            "[project]\nname = \"querykit\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n"
        } else {
            "[project]\nname = \"querykit\"\nversion = \"0.1.0\"\n"
        },
    )?;
    std::fs::write(
        producer_root.join("src/helpers.incn"),
        r#"pub model IntLiteralExpr:
  pub value: int

pub model StringLiteralExpr:
  pub value: str

pub type LiteralValue = Union[int, str]
pub type ColumnExpr = Union[IntLiteralExpr, StringLiteralExpr]

pub model AggregateMeasure:
  pub expr: ColumnExpr
  pub label: str

pub const DEFAULT_LABEL: str = "orders"
pub const COUNT_SENTINEL: str = "__querykit_count_no_argument__"

pub def lit(value: LiteralValue) -> ColumnExpr:
  match value:
    int(number) => return IntLiteralExpr(value=number)
    str(text) => return StringLiteralExpr(value=text)

pub def col(name: str) -> ColumnExpr:
  return StringLiteralExpr(value=name)

pub def count(expr: ColumnExpr = col(COUNT_SENTINEL)) -> ColumnExpr:
  return expr

pub def aggregate_as(expr: ColumnExpr, output_name: str) -> AggregateMeasure:
  return AggregateMeasure(expr=expr, label=output_name)

pub def aggregate_default(expr: ColumnExpr, output_name: str = DEFAULT_LABEL) -> AggregateMeasure:
  return AggregateMeasure(expr=expr, label=output_name)
"#,
    )?;
    std::fs::write(
        producer_root.join("src/lib.incn"),
        "pub from helpers import IntLiteralExpr, StringLiteralExpr, LiteralValue, ColumnExpr, AggregateMeasure, DEFAULT_LABEL, lit, count, aggregate_as, aggregate_default\n",
    )?;

    if with_vocab {
        write_vocab_companion_crate_with_source(
            &producer_root,
            "vocab_companion",
            "querykit_helper_vocab_companion",
            querykit_helper_vocab_companion_source(),
        )?;
    }

    let producer_build = bake_library_provider(&producer_root)?;
    assert!(
        producer_build.status.success(),
        "expected querykit producer build to succeed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&producer_build.stdout),
        String::from_utf8_lossy(&producer_build.stderr)
    );
    Ok(())
}

fn write_pub_library_with_provider_requirements(
    root: &Path,
    dependency_key: &str,
    manifest_name: &str,
    required_dependencies: Vec<incan_vocab::CargoDependency>,
    required_stdlib_features: Vec<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let artifact_root = root.join("deps").join(dependency_key).join("target").join("lib");
    std::fs::create_dir_all(artifact_root.join("src"))?;
    write_minimal_library_crate(&artifact_root, manifest_name)?;

    let mut manifest = LibraryManifest::new(manifest_name, "0.1.0");
    manifest.vocab = Some(incan_frontend::library_manifest::VocabExports {
        crate_path: format!("{dependency_key}_vocab_companion"),
        package_name: format!("{dependency_key}_vocab_companion"),
        keyword_registrations: Vec::new(),
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest {
            required_dependencies,
            required_stdlib_features: required_stdlib_features
                .into_iter()
                .map(std::string::ToString::to_string)
                .collect(),
            ..incan_vocab::LibraryManifest::default()
        },
        desugarer_artifact: None,
    });
    manifest.write_to_path(&artifact_root.join(format!("{manifest_name}.incnlib")))?;
    Ok(())
}

fn write_and_bake_source_provider_with_requirements_and_assert_keyword(
    root: &Path,
    axum_requirement: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let provider_root = root.join("deps/widgets");
    std::fs::create_dir_all(provider_root.join("src"))?;
    std::fs::write(
        provider_root.join("loaf.toml"),
        "[project]\nname = \"requirements_widgets_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
    )?;
    std::fs::write(
        provider_root.join("src/lib.incn"),
        "pub def requirements_fixture_identity() -> int:\n  return 1\n",
    )?;
    let companion_source = r#"use incan_vocab::{
    CargoDependency, CargoDependencySource, KeywordActivation, KeywordRegistration, KeywordSpec,
    KeywordSurfaceKind, LibraryManifest, VocabRegistration,
};

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_keyword_registration(KeywordRegistration {
            activation: KeywordActivation::OnImport {
                namespace: "widgets.dsl".to_string(),
            },
            keywords: vec![KeywordSpec::new("assert", KeywordSurfaceKind::ControlFlow)],
            valid_decorators: Vec::new(),
        })
        .with_library_manifest(LibraryManifest {
            required_dependencies: vec![CargoDependency {
                crate_name: "axum".to_string(),
                source: CargoDependencySource::Version("__AXUM_REQUIREMENT__".to_string()),
            }],
            required_stdlib_features: vec!["web".to_string()],
            ..LibraryManifest::default()
        })
}
"#
    .replace("__AXUM_REQUIREMENT__", axum_requirement);
    write_vocab_companion_crate_with_source(
        &provider_root,
        "vocab_companion",
        "requirements_widgets_vocab_companion",
        &companion_source,
    )?;
    let provider_bake = bake_library_provider(&provider_root)?;
    assert!(
        provider_bake.status.success(),
        "expected the widgets provider requirements to bake for axum {axum_requirement}.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&provider_bake.stdout),
        String::from_utf8_lossy(&provider_bake.stderr)
    );
    Ok(())
}

/// Publish the root identity a schema-v2 manifest owes for one directly declared model export.
///
/// A v2 identity graph publishes one root entry per raw declaration. A hand-built fixture that pushes a
/// `ModelExport` without one is rejected while the manifest is written, before the test reaches the diagnostic it
/// is actually checking.
fn push_root_model_identity(manifest: &mut LibraryManifest, library: &str, name: &str) {
    let identity = incan_semantics_core::CanonicalSymbolId {
        namespace: incan_semantics_core::SymbolNamespace::OrdinaryLexical,
        origin: incan_semantics_core::SymbolOrigin::Package {
            library: library.to_string(),
            module_path: Vec::new(),
        },
        declaration_name: name.to_string(),
        kind: incan_semantics_core::SemanticSourceTargetKind::Model,
        scope_discriminant: None,
        declaration_span: incan_semantics_core::HirSourceSpan::new(0, 1),
    };
    manifest
        .contract_metadata
        .identity_graph
        .exports
        .push(incan_frontend::library_manifest::ExportIdentity {
            public_name: name.to_string(),
            public_path: vec![library.to_string(), name.to_string()],
            source_path: vec![name.to_string()],
            kind: incan_frontend::library_manifest::ExportIdentityKind::Model,
            projection: incan_frontend::library_manifest::ExportIdentityProjection::Direct,
            canonical: incan_frontend::library_manifest::CanonicalIdentityExport::from_canonical(library, &identity),
        });
}

fn mylib_manifest_with_widget() -> LibraryManifest {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.exports.models.push(ModelExport {
        name: "Widget".to_string(),
        type_params: Vec::new(),
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
        fields: Vec::new(),
        properties: Vec::new(),
        methods: Vec::new(),
    });
    push_root_model_identity(&mut manifest, "mylib", "Widget");
    manifest
}

fn sqlite_checked_c_source(sqlite_header: &Path) -> String {
    format!(
        r#"from std.interop import c

binding SQLite:
    header = "{}"
    link = c.system_library("sqlite3")

    resource Database:
        native = "sqlite3"
        release = close

    enum Status:
        OK: c.i32 = SQLITE_OK

    symbol close(database: c.Owned[Database]) -> c.i32:
        native = "sqlite3_close"

    symbol open(path: c.ConstPtr[c.c_char], database: c.Out[c.Owned[Database]]) -> c.i32:
        native = "sqlite3_open"

        outcome Status.OK:
            initializes = [database]

    symbol error_message(database: c.Borrowed[Database]) -> c.ConstPtr[c.c_char]:
        native = "sqlite3_errmsg"

def memory_database_error_is_owned_text() -> Result[str, str]:
    path = c.cstr(":memory:")?
    unsafe:
        database_slot = c.out[c.Owned[Database]]()
        status = SQLite.open(path.as_const_ptr(), database_slot)
        if status == SQLite.Status.OK:
            database = database_slot.take()
            view = SQLite.error_message(database)
            text = view.copy_utf8(max_bytes=4096)?
            SQLite.close(database)
            return Ok(text)
        return Err("sqlite open failed")

def main() -> Result[None, str]:
    assert memory_database_error_is_owned_text()? != ""
"#,
        sqlite_header.display()
    )
}

fn sqlite_header_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    [
        "/usr/include/sqlite3.h",
        "/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/usr/include/sqlite3.h",
        "/opt/homebrew/include/sqlite3.h",
        "/usr/local/include/sqlite3.h",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
    .ok_or_else(|| "SQLite acceptance requires a system sqlite3.h header".into())
}
