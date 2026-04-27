//! Local toolchain inspection commands.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde_json::json;

use crate::cli::{CliError, CliResult, ExitCode};

/// Output format for `incan tools doctor`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolsDoctorFormat {
    /// Human-readable diagnostic report.
    Text,
    /// Machine-readable JSON report for editor integrations and issue templates.
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

/// Render an optional path, using a consistent placeholder when absent.
fn display_option_path(path: &Option<PathBuf>) -> String {
    path.as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "(not found)".to_string())
}
