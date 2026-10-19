//! Executable builds on the direct route: the program and its Loaf dependencies from source, their Rust facets from
//! the Oven native graph, compiled by the pinned driver (`incan-rustc-driver --source`).
//!
//! The native closure a program links (the standard-library facets, their Rust dependencies and
//! `incan_native_runtime`) comes from one prepared native-runtime Loaf: its library receipt selects the stored
//! direct-rustc plan whose externs and search paths the driver receives. Until the toolchain ships that Loaf and the
//! driver builds library units itself, `INCAN_NATIVE_RUNTIME_ROOT` names it.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use oven_rustc::plan::OvenStoredDirectRustcExecutionPlan;
use oven_rustc::plan::selection::select_receipt_direct_rustc_execution_plan;
use oven_rustc::rustc::{clear_inherited_cargo_environment, resolve_active_rustc, rustc_sysroot};
use oven_store::OvenReceipt;

use super::{BuildCommandOptions, RunProfile};
use crate::error::{CliError, CliResult};
use crate::oven_store::open_default_oven_store;
use crate::project::{discover_effective_project_manifest, resolve_project_root};

/// Names the prepared native-runtime Loaf root (a baked `incan_native_runtime`) until the toolchain ships one.
pub const NATIVE_RUNTIME_ROOT_ENV: &str = "INCAN_NATIVE_RUNTIME_ROOT";

/// Names the `incan-rustc-driver` executable when it does not sit beside `incan`.
pub const RUSTC_DRIVER_ENV: &str = "INCAN_RUSTC_DRIVER";

/// The externs and search paths one program links, held with the store lease that keeps them in place.
struct NativeClosure {
    externs: Vec<(String, PathBuf)>,
    directories: Vec<PathBuf>,
    /// The selected plan owns the execution lease; the closure's paths stay valid while it is alive.
    _plan: OvenStoredDirectRustcExecutionPlan,
}

/// Compile one executable through the direct route and report its binary.
///
/// Completed-output reuse has already missed when this is called. The output lands at `OUTPUT_DIR/<name>` when an
/// output directory is given, otherwise at `<project>/target/incan/<name>/<profile>/<name>`.
pub(super) fn build(
    file_path: &str,
    output_dir: Option<&String>,
    options: &BuildCommandOptions,
    progress: &mut impl FnMut(String),
) -> CliResult<serde_json::Value> {
    let source = fs::canonicalize(file_path)
        .map_err(|error| CliError::failure(format!("cannot read the program source {file_path}: {error}")))?;
    let project_root = resolve_project_root(&source);
    let name = executable_name(&project_root, &source)?;
    let profile = match options.executable_profile {
        RunProfile::Debug => "debug",
        RunProfile::Release => "release",
    };
    let output_directory = match output_dir {
        Some(directory) => PathBuf::from(directory),
        None => project_root.join("target/incan").join(&name).join(profile),
    };
    fs::create_dir_all(&output_directory).map_err(|error| {
        CliError::failure(format!(
            "cannot create the output directory {}: {error}",
            output_directory.display()
        ))
    })?;
    let output = output_directory.join(&name);

    // ---- The pinned toolchain, the driver and the program's native closure ----
    let rustc = resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
    let sysroot = rustc_sysroot(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let driver = rustc_driver()?;
    let closure = native_closure(profile)?;

    // ---- Compile ----
    progress(format!("Compiling {} ({profile})", source.display()));
    let mut command = Command::new(&driver);
    command
        .arg("--source")
        .arg(&source)
        .arg(crate_name(&name))
        .arg(&output)
        .arg(&sysroot);
    for (crate_name, artifact) in &closure.externs {
        command.arg("--extern").arg(format!("{crate_name}={}", artifact.display()));
    }
    for directory in &closure.directories {
        command.arg("--search").arg(directory);
    }
    clear_inherited_cargo_environment(&mut command);
    let compiled = command
        .output()
        .map_err(|error| CliError::failure(format!("cannot start {}: {error}", driver.display())))?;
    if !compiled.status.success() {
        return Err(CliError::failure(format!(
            "{} failed for {}:\n{}{}",
            driver.display(),
            source.display(),
            String::from_utf8_lossy(&compiled.stdout),
            String::from_utf8_lossy(&compiled.stderr)
        )));
    }
    progress(format!("Built {}", output.display()));
    Ok(serde_json::json!({
        "status": "success",
        "mode": "executable",
        "profile": profile,
        "entrypoint": source,
        "project": { "project_root": project_root },
        "artifacts": [{ "kind": "binary", "path": output }],
    }))
}

/// The executable's name: the manifest's project name when there is one, otherwise the source file's stem.
fn executable_name(project_root: &Path, source: &Path) -> CliResult<String> {
    let manifest = discover_effective_project_manifest(project_root)?;
    if let Some(name) = manifest
        .as_ref()
        .and_then(|manifest| manifest.project.as_ref())
        .and_then(|project| project.name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        return Ok(name.to_string());
    }
    source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
        .ok_or_else(|| CliError::failure(format!("{} has no usable file name", source.display())))
}

/// A Rust crate name for the program: ASCII alphanumerics and `_`, never starting with a digit.
fn crate_name(name: &str) -> String {
    let mut crate_name = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    if crate_name.is_empty() || crate_name.starts_with(|character: char| character.is_ascii_digit()) {
        crate_name.insert(0, '_');
    }
    crate_name
}

/// Find `incan-rustc-driver`: `INCAN_RUSTC_DRIVER` when set, otherwise beside the running `incan`.
fn rustc_driver() -> CliResult<PathBuf> {
    if let Some(driver) = env::var_os(RUSTC_DRIVER_ENV).filter(|value| !value.is_empty()) {
        let driver = PathBuf::from(driver);
        if !driver.is_file() {
            return Err(CliError::failure(format!(
                "{RUSTC_DRIVER_ENV} names {}, which is not a file",
                driver.display()
            )));
        }
        return Ok(driver);
    }
    let executable = env::current_exe()
        .map_err(|error| CliError::failure(format!("cannot locate the running incan executable: {error}")))?;
    let executable = fs::canonicalize(&executable).unwrap_or(executable);
    let driver = executable.with_file_name("incan-rustc-driver");
    if driver.is_file() {
        return Ok(driver);
    }
    Err(CliError::failure(format!(
        "no incan-rustc-driver beside {} (set {RUSTC_DRIVER_ENV} to its path)",
        executable.display()
    )))
}

/// Select the prepared native runtime's stored plan for `profile` and add the runtime library itself.
fn native_closure(profile: &str) -> CliResult<NativeClosure> {
    let root = env::var_os(NATIVE_RUNTIME_ROOT_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            CliError::failure(format!(
                "no prepared native runtime: set {NATIVE_RUNTIME_ROOT_ENV} to a baked incan_native_runtime Loaf"
            ))
        })?;
    let receipt_path = root.join(format!(".incan/oven/library-{profile}-receipt.json"));
    let receipt_bytes = fs::read(&receipt_path).map_err(|error| {
        CliError::failure(format!(
            "cannot read the native runtime receipt {}: {error}",
            receipt_path.display()
        ))
    })?;
    let receipt: OvenReceipt = serde_json::from_slice(&receipt_bytes).map_err(|error| {
        CliError::failure(format!(
            "the native runtime receipt {} is not valid: {error}",
            receipt_path.display()
        ))
    })?;
    let store = open_default_oven_store()?;
    let plan = select_receipt_direct_rustc_execution_plan(&store, &receipt)
        .map_err(|error| CliError::failure(error.to_string()))?
        .ok_or_else(|| {
            CliError::failure(format!(
                "the Oven store {} holds no plan for the native runtime receipt {}",
                store.root().display(),
                receipt_path.display()
            ))
        })?;
    let runtime_library = root
        .join("target/lib/oven")
        .join(profile)
        .join("libincan_native_runtime.rlib");
    if !runtime_library.is_file() {
        return Err(CliError::failure(format!(
            "the native runtime library {} is missing",
            runtime_library.display()
        )));
    }
    let mut externs = plan.artifact_plan.externs.clone();
    externs.push(("incan_native_runtime".to_string(), runtime_library));
    Ok(NativeClosure {
        externs,
        directories: plan.artifact_plan.dependency_search_paths.clone(),
        _plan: plan,
    })
}
