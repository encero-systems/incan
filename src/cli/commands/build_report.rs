//! Build report and generated Rust inspection payloads.
//!
//! These structs are the stable, machine-readable projection of the existing build and code generation paths. They
//! deliberately describe current emitted artifacts without making generated Rust a stable ABI.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use clap::ValueEnum;
use serde::Serialize;

use crate::cli::{CliError, CliResult};
use crate::dependency_resolver::InlineRustImport;
use crate::manifest::{DependencySource, DependencySpec, GitReference, LibraryDependencySpec};
use crate::version::INCAN_VERSION;

use super::common::CargoPolicy;

/// Schema version for build and generated Rust inspection reports.
pub(crate) const BUILD_REPORT_SCHEMA_VERSION: u32 = 1;

/// Build report output format.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildReportFormat {
    Json,
}

/// Options that control optional build report emission.
#[derive(Debug, Clone, Default)]
pub struct BuildReportOptions {
    pub format: Option<BuildReportFormat>,
    pub output_path: Option<PathBuf>,
    /// Compiler-owned collector for a workspace command that emits one aggregate report after all member builds.
    captured_reports: Option<Arc<Mutex<Vec<BuildReport>>>>,
}

impl BuildReportOptions {
    /// Construct CLI report options without exposing compiler-internal capture state.
    pub fn new(format: Option<BuildReportFormat>, output_path: Option<PathBuf>) -> Self {
        Self {
            format,
            output_path,
            captured_reports: None,
        }
    }

    /// Return whether the caller requested any build report output.
    pub(crate) fn enabled(&self) -> bool {
        self.format.is_some()
    }

    /// Preserve normal report formatting semantics while collecting a completed member report instead of emitting it.
    pub(crate) fn with_capture(&self, captured_reports: Arc<Mutex<Vec<BuildReport>>>) -> Self {
        Self {
            format: self.format,
            output_path: self.output_path.clone(),
            captured_reports: Some(captured_reports),
        }
    }
}

/// Generated Rust inspection output format.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustInspectionFormat {
    Text,
    Json,
}

/// High-level build mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildReportMode {
    Executable,
    Library,
}

/// Build status recorded in a successful report payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildReportStatus {
    Success,
}

/// Project identity included in reports.
#[derive(Debug, Clone, Serialize)]
pub struct BuildReportProject {
    pub name: String,
    pub version: Option<String>,
    pub project_root: String,
}

/// Generated Rust project locations.
#[derive(Debug, Clone, Serialize)]
pub struct GeneratedRustProjectReport {
    pub project_path: String,
    pub manifest_path: String,
    pub crate_root: String,
    pub cargo_target_dir: String,
}

/// Source module that contributed to generated output.
#[derive(Debug, Clone, Serialize)]
pub struct SourceFileReport {
    pub path: String,
    pub module_path: Vec<String>,
}

/// Emitted artifact location and cheap filesystem metadata.
#[derive(Debug, Clone, Serialize)]
pub struct BuildArtifactReport {
    pub kind: String,
    pub path: String,
    pub exists: bool,
    pub size_bytes: Option<u64>,
}

/// Rust dependency summary.
#[derive(Debug, Clone, Serialize)]
pub struct RustDependencyReport {
    pub crate_name: String,
    pub package: Option<String>,
    pub version: Option<String>,
    pub source: String,
    pub source_detail: Option<String>,
    pub features: Vec<String>,
    pub default_features: bool,
    pub optional: bool,
}

/// Incan dependency summary.
#[derive(Debug, Clone, Serialize)]
pub struct IncanDependencyReport {
    pub library_name: String,
    pub path: String,
}

/// Dependencies and stdlib features involved in the build.
#[derive(Debug, Clone, Serialize)]
pub struct BuildDependencyReport {
    pub rust: Vec<RustDependencyReport>,
    pub rust_dev: Vec<RustDependencyReport>,
    pub incan: Vec<IncanDependencyReport>,
    pub stdlib_features: Vec<String>,
}

/// Cargo feature and policy flags for the generated build.
#[derive(Debug, Clone, Serialize)]
pub struct BuildCargoReport {
    pub offline: bool,
    pub locked: bool,
    pub frozen: bool,
    pub extra_args: Vec<String>,
    pub features: Vec<String>,
    pub no_default_features: bool,
    pub all_features: bool,
}

/// Rust interop use summary.
#[derive(Debug, Clone, Serialize)]
pub struct BuildInteropReport {
    pub rust_imports: Vec<String>,
    pub rust_externs: Vec<String>,
    pub rust_abi_query_paths: Vec<String>,
}

/// Versioned build report.
#[derive(Debug, Clone, Serialize)]
pub struct BuildReport {
    pub schema_version: u32,
    pub compiler_version: &'static str,
    pub status: BuildReportStatus,
    pub mode: BuildReportMode,
    pub profile: String,
    pub project: BuildReportProject,
    pub entrypoint: Option<String>,
    pub library_root: Option<String>,
    pub source_files: Vec<SourceFileReport>,
    pub generated: GeneratedRustProjectReport,
    pub artifacts: Vec<BuildArtifactReport>,
    pub dependencies: BuildDependencyReport,
    pub cargo: BuildCargoReport,
    pub interop: BuildInteropReport,
    pub timings_ms: BTreeMap<String, u64>,
    pub notes: Vec<String>,
}

/// Build report data collected before the actual Cargo build finishes.
#[derive(Debug, Clone)]
pub struct BuildReportDraft {
    pub mode: BuildReportMode,
    pub profile: String,
    pub project: BuildReportProject,
    pub entrypoint: Option<String>,
    pub library_root: Option<String>,
    pub source_files: Vec<SourceFileReport>,
    pub generated: GeneratedRustProjectReport,
    pub artifacts: Vec<BuildArtifactReport>,
    pub dependencies: BuildDependencyReport,
    pub cargo: BuildCargoReport,
    pub interop: BuildInteropReport,
    pub notes: Vec<String>,
}

impl BuildReportDraft {
    /// Complete the draft with final timings and version/status fields after Cargo has finished successfully.
    pub(crate) fn finish(self, timings_ms: BTreeMap<String, u64>) -> BuildReport {
        BuildReport {
            schema_version: BUILD_REPORT_SCHEMA_VERSION,
            compiler_version: INCAN_VERSION,
            status: BuildReportStatus::Success,
            mode: self.mode,
            profile: self.profile,
            project: self.project,
            entrypoint: self.entrypoint,
            library_root: self.library_root,
            source_files: self.source_files,
            generated: self.generated,
            artifacts: self.artifacts,
            dependencies: self.dependencies,
            cargo: self.cargo,
            interop: self.interop,
            timings_ms,
            notes: self.notes,
        }
    }
}

/// Versioned generated Rust inspection report.
#[derive(Debug, Clone, Serialize)]
pub struct RustInspectionReport {
    pub schema_version: u32,
    pub compiler_version: &'static str,
    pub mode: BuildReportMode,
    pub generated: GeneratedRustProjectReport,
    pub source_files: Vec<SourceFileReport>,
    pub rust_files: Vec<RustFileReport>,
    pub notes: Vec<String>,
}

/// One emitted Rust source file.
#[derive(Debug, Clone, Serialize)]
pub struct RustFileReport {
    pub path: String,
    pub size_bytes: u64,
    pub crate_root: bool,
}

/// Write or print a build report according to CLI options.
pub(crate) fn emit_build_report(report: &BuildReport, options: &BuildReportOptions) -> CliResult<()> {
    if let Some(captured_reports) = &options.captured_reports {
        captured_reports
            .lock()
            .map_err(|_| CliError::failure("failed to lock captured workspace build reports"))?
            .push(report.clone());
        return Ok(());
    }
    let Some(format) = options.format else {
        return Ok(());
    };
    match format {
        BuildReportFormat::Json => {
            let json = serde_json::to_string_pretty(report)
                .map_err(|error| CliError::failure(format!("failed to serialize build report JSON: {error}")))?;
            if let Some(path) = &options.output_path {
                write_json_file(path, &json)?;
            } else {
                println!("{json}");
            }
        }
    }
    Ok(())
}

/// Render generated Rust inspection output.
pub(crate) fn emit_rust_inspection_report(
    report: &RustInspectionReport,
    format: RustInspectionFormat,
) -> CliResult<()> {
    match format {
        RustInspectionFormat::Text => {
            println!("Generated Rust project: {}", report.generated.project_path);
            println!("Crate root: {}", report.generated.crate_root);
            println!("Rust files:");
            for file in &report.rust_files {
                println!("- {}", file.path);
            }
        }
        RustInspectionFormat::Json => {
            let json = serde_json::to_string_pretty(report).map_err(|error| {
                CliError::failure(format!("failed to serialize generated Rust inspection JSON: {error}"))
            })?;
            println!("{json}");
        }
    }
    Ok(())
}

/// Build the generated Rust project location block shared by build and inspection reports.
pub(crate) fn generated_project_report(
    project_path: &Path,
    crate_root: &Path,
    cargo_target_dir: &Path,
) -> GeneratedRustProjectReport {
    GeneratedRustProjectReport {
        project_path: path_string(project_path),
        manifest_path: path_string(&project_path.join("Cargo.toml")),
        crate_root: path_string(crate_root),
        cargo_target_dir: path_string(cargo_target_dir),
    }
}

/// Summarize one emitted artifact path and whether it exists after the build step that should create it.
pub(crate) fn artifact_report(kind: impl Into<String>, path: &Path) -> BuildArtifactReport {
    let metadata = fs::metadata(path).ok();
    BuildArtifactReport {
        kind: kind.into(),
        path: path_string(path),
        exists: metadata.is_some(),
        size_bytes: metadata.map(|metadata| metadata.len()),
    }
}

/// Convert the resolved dependency surfaces into the stable dependency block used by build reports.
pub(crate) fn dependencies_report(
    rust: &[DependencySpec],
    rust_dev: &[DependencySpec],
    incan: Vec<IncanDependencyReport>,
    stdlib_features: Vec<String>,
) -> BuildDependencyReport {
    BuildDependencyReport {
        rust: rust.iter().map(rust_dependency_report).collect(),
        rust_dev: rust_dev.iter().map(rust_dependency_report).collect(),
        incan,
        stdlib_features,
    }
}

/// Convert sorted Incan library dependencies into report rows.
pub(crate) fn incan_dependencies_report(
    dependencies: Vec<(&String, &LibraryDependencySpec)>,
) -> Vec<IncanDependencyReport> {
    let mut report = dependencies
        .into_iter()
        .map(|(key, dependency)| IncanDependencyReport {
            library_name: key.clone(),
            path: path_string(&dependency.path),
        })
        .collect::<Vec<_>>();
    report.sort_by(|left, right| left.library_name.cmp(&right.library_name));
    report
}

/// Capture Cargo policy flags and feature selections exactly as the CLI forwarded them to Cargo.
pub(crate) fn cargo_report(
    policy: &CargoPolicy,
    features: Vec<String>,
    no_default_features: bool,
    all_features: bool,
) -> BuildCargoReport {
    BuildCargoReport {
        offline: policy.offline,
        locked: policy.locked,
        frozen: policy.frozen,
        extra_args: policy.extra_args.clone(),
        features,
        no_default_features,
        all_features,
    }
}

/// Summarize Rust interop surfaces referenced by source code and inferred ABI query paths.
pub(crate) fn interop_report(
    imports: &[InlineRustImport],
    rust_externs: Vec<String>,
    rust_abi_query_paths: Vec<String>,
) -> BuildInteropReport {
    let mut rust_imports = imports
        .iter()
        .map(|import| import.import_path.clone())
        .collect::<Vec<_>>();
    rust_imports.sort();
    rust_imports.dedup();
    BuildInteropReport {
        rust_imports,
        rust_externs,
        rust_abi_query_paths,
    }
}

/// Collect generated Rust files and package them with source breadcrumbs for `incan inspect rust`.
pub(crate) fn rust_inspection_report(
    mode: BuildReportMode,
    generated: GeneratedRustProjectReport,
    source_files: Vec<SourceFileReport>,
    notes: Vec<String>,
) -> CliResult<RustInspectionReport> {
    let crate_root = PathBuf::from(&generated.crate_root);
    let project_path = PathBuf::from(&generated.project_path);
    let src_dir = project_path.join("src");
    let mut rust_files = Vec::new();
    collect_rust_files(&src_dir, &crate_root, &mut rust_files)?;
    rust_files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(RustInspectionReport {
        schema_version: BUILD_REPORT_SCHEMA_VERSION,
        compiler_version: INCAN_VERSION,
        mode,
        generated,
        source_files,
        rust_files,
        notes,
    })
}

/// Convert a filesystem path into the lossy UTF-8 string form used by JSON reports.
pub(crate) fn path_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

/// Convert one manifest Rust dependency into a stable report row.
fn rust_dependency_report(spec: &DependencySpec) -> RustDependencyReport {
    let (source, source_detail) = match &spec.source {
        DependencySource::Registry => ("registry".to_string(), None),
        DependencySource::Git { url, reference } => {
            ("git".to_string(), Some(format!("{url}#{}", git_reference(reference))))
        }
        DependencySource::Path { path } => ("path".to_string(), Some(path_string(path))),
    };
    RustDependencyReport {
        crate_name: spec.crate_name.clone(),
        package: spec.package.clone(),
        version: spec.version.clone(),
        source,
        source_detail,
        features: spec.features.clone(),
        default_features: spec.default_features,
        optional: spec.optional,
    }
}

/// Render one git dependency reference for report output.
fn git_reference(reference: &GitReference) -> String {
    match reference {
        GitReference::Branch(branch) => format!("branch={branch}"),
        GitReference::Tag(tag) => format!("tag={tag}"),
        GitReference::Rev(rev) => format!("rev={rev}"),
    }
}

/// Recursively collect generated Rust source files below `dir`.
fn collect_rust_files(dir: &Path, crate_root: &Path, output: &mut Vec<RustFileReport>) -> CliResult<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|error| {
        CliError::failure(format!(
            "failed to read generated Rust directory {}: {error}",
            dir.display()
        ))
    })? {
        let entry = entry
            .map_err(|error| CliError::failure(format!("failed to read generated Rust directory entry: {error}")))?;
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, crate_root, output)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let metadata = entry.metadata().map_err(|error| {
                CliError::failure(format!(
                    "failed to stat generated Rust file {}: {error}",
                    path.display()
                ))
            })?;
            output.push(RustFileReport {
                path: path_string(&path),
                size_bytes: metadata.len(),
                crate_root: path == crate_root,
            });
        }
    }
    Ok(())
}

/// Write pretty JSON to disk with a trailing newline for friendly diffs and shell output.
fn write_json_file(path: &Path, json: &str) -> CliResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            CliError::failure(format!(
                "failed to create report output directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    fs::write(path, format!("{json}\n"))
        .map_err(|error| CliError::failure(format!("failed to write report output {}: {error}", path.display())))
}
