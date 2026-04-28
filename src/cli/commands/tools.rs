//! Local toolchain inspection commands.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde_json::json;

use crate::cli::prelude::ParsedModule;
use crate::cli::{CliError, CliResult, ExitCode};
use crate::frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
};
use crate::frontend::diagnostics;
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::typechecker;
use crate::manifest::ProjectManifest;

use super::common::{collect_modules, imported_module_deps_for_with_index, module_key_index, resolve_project_root};

/// Output format for `incan tools doctor`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolsDoctorFormat {
    /// Human-readable diagnostic report.
    Text,
    /// Machine-readable JSON report for editor integrations and issue templates.
    Json,
}

/// Output format for `incan tools metadata api`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolsMetadataFormat {
    /// Stable checked API metadata JSON.
    Json,
}

/// Run local toolchain diagnostics for CLI and editor setup.
pub fn tools_doctor(format: ToolsDoctorFormat) -> CliResult<ExitCode> {
    let report = DoctorReport::collect();
    match format {
        ToolsDoctorFormat::Text => report.print_text(),
        ToolsDoctorFormat::Json => report.print_json()?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Emit checked public API metadata for a source file or project directory.
pub fn tools_metadata_api(path: &Path, format: ToolsMetadataFormat) -> CliResult<ExitCode> {
    let package = collect_api_metadata_package(path)?;
    match format {
        ToolsMetadataFormat::Json => {
            let output = serde_json::to_string_pretty(&package)
                .map_err(|error| CliError::failure(format!("failed to serialize API metadata: {error}")))?;
            println!("{output}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Type-check a metadata entry path and collect checked API metadata for all local modules.
fn collect_api_metadata_package(path: &Path) -> CliResult<CheckedApiMetadataPackage> {
    let entry_path = resolve_metadata_entry_path(path)?;
    let entry_path_string = entry_path.to_string_lossy();
    let modules = collect_modules(&entry_path_string)?;
    let project_root = resolve_project_root(&entry_path);
    let manifest = ProjectManifest::discover(&project_root).map_err(|error| CliError::failure(error.to_string()))?;
    let declared = manifest.as_ref().map(ProjectManifest::declared_rust_crate_names);
    let library_manifest_index = manifest
        .as_ref()
        .map(LibraryManifestIndex::from_project_manifest)
        .unwrap_or_default();
    let module_idx_by_key = module_key_index(&modules);
    let mut all_errors = String::new();
    let mut metadata_modules = Vec::new();

    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
        let mut checker = typechecker::TypeChecker::new();
        if let Some(names) = declared.clone() {
            checker.set_declared_crate_names(names);
        }
        checker.set_library_manifest_index(library_manifest_index.clone());

        match checker.check_with_imports(&module.ast, &deps_for_module) {
            Ok(()) => {
                for warn in checker.warnings() {
                    eprint!(
                        "{}",
                        diagnostics::format_error(module.file_path.to_string_lossy().as_ref(), &module.source, warn)
                    );
                }
                metadata_modules.push(collect_checked_api_metadata(
                    &module.ast,
                    &checker,
                    metadata_module_path(module, &entry_path),
                ));
            }
            Err(errs) => {
                for err in &errs {
                    all_errors.push_str(&diagnostics::format_error(
                        module.file_path.to_string_lossy().as_ref(),
                        &module.source,
                        err,
                    ));
                }
            }
        }
    }

    if !all_errors.is_empty() {
        return Err(CliError::failure(all_errors.trim_end()));
    }

    Ok(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        modules: metadata_modules,
    })
}

/// Return the logical module path used in metadata for one parsed module.
fn metadata_module_path(module: &ParsedModule, entry_path: &Path) -> Vec<String> {
    if module.file_path == entry_path
        && let Some(stem) = entry_path.file_stem().and_then(|stem| stem.to_str())
    {
        return vec![stem.to_string()];
    }
    module.path_segments.clone()
}

/// Resolve a file or project directory to the source file used as the metadata entry point.
fn resolve_metadata_entry_path(path: &Path) -> CliResult<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?
            .join(path)
    };

    if absolute.is_file() {
        return Ok(absolute);
    }
    if absolute.is_dir() {
        let lib = absolute.join("src").join("lib.incn");
        if lib.is_file() {
            return Ok(lib);
        }
        let main = absolute.join("src").join("main.incn");
        if main.is_file() {
            return Ok(main);
        }
        return Err(CliError::failure(format!(
            "metadata API extraction requires an Incan source file, or a project directory with `src/lib.incn` or `src/main.incn`: {}",
            absolute.display()
        )));
    }

    Err(CliError::failure(format!(
        "metadata API extraction path does not exist: {}",
        absolute.display()
    )))
}

#[derive(Debug)]
struct DoctorReport {
    version: &'static str,
    current_exe: Option<PathBuf>,
    cwd: Option<PathBuf>,
    path_incan: ToolPath,
    path_incan_lsp: ToolPath,
    cargo_bin_incan: CargoBinEntry,
    cargo_bin_incan_lsp: CargoBinEntry,
}

impl DoctorReport {
    /// Collect local process, PATH, and cargo-bin state for the doctor report.
    fn collect() -> Self {
        Self {
            version: crate::version::INCAN_VERSION,
            current_exe: env::current_exe().ok(),
            cwd: env::current_dir().ok(),
            path_incan: ToolPath::resolve("incan"),
            path_incan_lsp: ToolPath::resolve("incan-lsp"),
            cargo_bin_incan: CargoBinEntry::from_home("incan"),
            cargo_bin_incan_lsp: CargoBinEntry::from_home("incan-lsp"),
        }
    }

    /// Print the doctor report as stable, human-readable text.
    fn print_text(&self) {
        println!("Incan tools doctor");
        println!("version: {}", self.version);
        println!("current_exe: {}", display_option_path(&self.current_exe));
        println!("cwd: {}", display_option_path(&self.cwd));
        println!();
        self.path_incan.print_text("PATH incan");
        self.path_incan_lsp.print_text("PATH incan-lsp");
        println!();
        self.cargo_bin_incan.print_text("~/.cargo/bin/incan");
        self.cargo_bin_incan_lsp.print_text("~/.cargo/bin/incan-lsp");
        println!();
        println!("editor setup:");
        println!("  leave incan.lsp.path and incan.compiler.path empty to use workspace discovery or PATH");
        println!(
            "  if either setting is explicit, use a literal executable path; shell syntax like $HOME or ~ is not expanded"
        );
        println!("  after rebuilding or changing paths, reload VS Code/Cursor so it starts a fresh incan-lsp process");
    }

    /// Print the doctor report as pretty JSON for editor integrations and issue templates.
    fn print_json(&self) -> CliResult<()> {
        let value = json!({
            "version": self.version,
            "current_exe": self.current_exe.as_deref().map(path_to_string),
            "cwd": self.cwd.as_deref().map(path_to_string),
            "path": {
                "incan": self.path_incan.as_json(),
                "incan_lsp": self.path_incan_lsp.as_json(),
            },
            "cargo_bin": {
                "incan": self.cargo_bin_incan.as_json(),
                "incan_lsp": self.cargo_bin_incan_lsp.as_json(),
            },
            "editor_setup": {
                "recommended_lsp_path": "",
                "recommended_compiler_path": "",
                "literal_path_settings": true,
                "reload_after_rebuild": true
            }
        });
        let output = serde_json::to_string_pretty(&value)
            .map_err(|error| CliError::failure(format!("failed to serialize doctor report: {error}")))?;
        println!("{output}");
        Ok(())
    }
}

#[derive(Debug)]
struct ToolPath {
    command: String,
    resolved: Option<PathBuf>,
    executable: bool,
}

impl ToolPath {
    /// Resolve one command name through the current process PATH.
    fn resolve(command: &str) -> Self {
        let resolved = find_on_path(command);
        let executable = resolved.as_deref().is_some_and(is_executable_file);
        Self {
            command: command.to_string(),
            resolved,
            executable,
        }
    }

    /// Print one PATH resolution entry.
    fn print_text(&self, label: &str) {
        println!("{label}:");
        println!("  command: {}", self.command);
        println!("  resolved: {}", display_option_path(&self.resolved));
        println!("  executable: {}", self.executable);
    }

    /// Convert one PATH resolution entry into JSON.
    fn as_json(&self) -> serde_json::Value {
        json!({
            "command": self.command,
            "resolved": self.resolved.as_deref().map(path_to_string),
            "executable": self.executable,
        })
    }
}

#[derive(Debug)]
struct CargoBinEntry {
    path: Option<PathBuf>,
    exists: bool,
    symlink_target: Option<PathBuf>,
    executable: bool,
}

impl CargoBinEntry {
    /// Inspect one expected `~/.cargo/bin` tool entry.
    fn from_home(binary: &str) -> Self {
        let path = home_dir().map(|home| home.join(".cargo").join("bin").join(binary));
        let exists = path.as_deref().is_some_and(Path::exists);
        let symlink_target = path.as_deref().and_then(|path| fs::read_link(path).ok());
        let executable = path.as_deref().is_some_and(is_executable_file);
        Self {
            path,
            exists,
            symlink_target,
            executable,
        }
    }

    /// Print one cargo-bin entry.
    fn print_text(&self, label: &str) {
        println!("{label}:");
        println!("  path: {}", display_option_path(&self.path));
        println!("  exists: {}", self.exists);
        println!("  symlink_target: {}", display_option_path(&self.symlink_target));
        println!("  executable: {}", self.executable);
    }

    /// Convert one cargo-bin entry into JSON.
    fn as_json(&self) -> serde_json::Value {
        json!({
            "path": self.path.as_deref().map(path_to_string),
            "exists": self.exists,
            "symlink_target": self.symlink_target.as_deref().map(path_to_string),
            "executable": self.executable,
        })
    }
}

/// Resolve the current user's home directory from platform-standard environment variables.
fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(PathBuf::from))
}

/// Find an executable command in the current process PATH.
fn find_on_path(command: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    for dir in env::split_paths(&paths) {
        for candidate in executable_candidates(&dir, command) {
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Build platform-specific executable candidates for one PATH directory.
fn executable_candidates(dir: &Path, command: &str) -> Vec<PathBuf> {
    if cfg!(windows) {
        let extensions = env::var_os("PATHEXT")
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".EXE;.CMD;.BAT;.COM".to_string());
        extensions
            .split(';')
            .map(|extension| dir.join(format!("{command}{extension}")))
            .collect()
    } else {
        vec![dir.join(command)]
    }
}

#[cfg(unix)]
/// Return whether a path is a regular executable file on Unix.
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
/// Return whether a path is an executable-like file on non-Unix platforms.
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

/// Render a path for plain text or JSON output.
fn path_to_string(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_api_metadata_package_extracts_project_lib() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let src = tmp.path().join("src");
        fs::create_dir_all(&src)?;
        fs::write(
            tmp.path().join("incan.toml"),
            r#"
[project]
name = "metadata_demo"
version = "0.1.0"
"#,
        )?;
        fs::write(
            src.join("lib.incn"),
            r#"
pub const LABEL = "demo"

pub def label() -> str:
    return LABEL
"#,
        )?;

        let package = collect_api_metadata_package(tmp.path())?;
        assert_eq!(package.schema_version, CHECKED_API_METADATA_SCHEMA_VERSION);
        assert_eq!(package.modules.len(), 1);
        assert_eq!(package.modules[0].module_path, vec!["lib".to_string()]);
        assert_eq!(package.modules[0].declarations.len(), 2);
        Ok(())
    }
}

/// Render an optional path, using a consistent placeholder when absent.
fn display_option_path(path: &Option<PathBuf>) -> String {
    path.as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "(not found)".to_string())
}
