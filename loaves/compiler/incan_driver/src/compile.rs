//! Compiling one unit through the pinned driver (`incan-rustc-driver --source`): an executable or a library Loaf
//! (DD-0007).
//!
//! Every Loaf a unit imports is compiled before the unit is checked (DD-0005). A standard package or path dependency
//! without an artifact for the profile is compiled first, as a library unit, and the unit then links each indexed
//! Loaf's rlib with that Loaf's native closure.
//!
//! The Rust facets, their Rust dependencies and `incan_native_runtime` come from one prepared native-runtime Loaf: its
//! library receipt selects the stored rustc plan whose externs and search paths every unit receives. Until the
//! toolchain ships that Loaf, `INCAN_NATIVE_RUNTIME_ROOT` names it.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use incan_frontend::loaf_index::artifact_dir;
use incan_lang::lang::standard_packages::STANDARD_PACKAGE_NAMESPACE_POLICIES;
use oven_model::manifest::{LOAF_MANIFEST_FILENAME, ProjectManifest};
use oven_model::toolchain_layout::CompilerOwnedSourceLayout;
use oven_rustc::plan::OvenStoredDirectRustcExecutionPlan;
use oven_rustc::plan::selection::select_receipt_direct_rustc_execution_plan;
use oven_rustc::rustc::{clear_inherited_cargo_environment, resolve_active_rustc, rustc_sysroot};
use oven_store::OvenReceipt;

use crate::error::{CliError, CliResult};
use crate::features::FeatureSelection;
use crate::oven_store::open_default_oven_store;
use crate::project::{discover_effective_project_manifest, resolve_project_root, resolve_source_root};
use crate::session::CompilationSession;

/// Names the prepared native-runtime Loaf root (a baked `incan_native_runtime`) until the toolchain ships one.
pub const NATIVE_RUNTIME_ROOT_ENV: &str = "INCAN_NATIVE_RUNTIME_ROOT";

/// Names the `incan-rustc-driver` executable when it does not sit beside `incan`.
pub const RUSTC_DRIVER_ENV: &str = "INCAN_RUSTC_DRIVER";

/// What a unit compiles to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    /// A program with a `main`.
    Executable,
    /// A Loaf: its library unit and the artifact directory DD-0007 describes.
    Library,
}

/// One unit to compile.
pub struct UnitRequest<'a> {
    /// The unit's entry source: a program's entry file, or a Loaf's `lib.incn`.
    pub source: &'a Path,
    /// What the unit compiles to.
    pub kind: UnitKind,
    /// The build profile, which selects the Loaf artifacts the unit reads and writes.
    pub profile: &'a str,
    /// The root package's feature selection.
    pub features: &'a FeatureSelection,
    /// Where an executable lands, instead of `<project>/target/incan/<name>/<profile>/`.
    pub output_dir: Option<&'a Path>,
}

/// A compiled unit.
pub struct UnitOutput {
    /// The executable, or the Loaf's rlib inside its artifact directory.
    pub artifact: PathBuf,
    /// The unit's project root.
    pub project_root: PathBuf,
    /// The unit's name: the project name, or the entry file's stem.
    pub name: String,
}

/// Compile one unit, first compiling every Loaf it reaches that has no artifact for the profile.
pub fn compile_unit(request: &UnitRequest<'_>, progress: &mut impl FnMut(String)) -> CliResult<UnitOutput> {
    let source = fs::canonicalize(request.source).map_err(|error| {
        CliError::failure(format!("cannot read the unit source {}: {error}", request.source.display()))
    })?;
    let manifest = discover_effective_project_manifest(&resolve_project_root(&source))?;
    prepare_loaves(manifest.as_ref(), request.profile, progress)?;
    compile_prepared_unit(&source, request, progress)
}

/// Compile one unit whose Loaves are already prepared.
fn compile_prepared_unit(
    source: &Path,
    request: &UnitRequest<'_>,
    progress: &mut impl FnMut(String),
) -> CliResult<UnitOutput> {
    let session = CompilationSession::discover(source, request.features, request.profile)?;
    let project_root = session
        .manifest
        .as_ref()
        .map(|manifest| manifest.project_root().to_path_buf())
        .unwrap_or_else(|| resolve_project_root(source));
    let name = unit_name(session.manifest.as_ref(), source)?;
    let output = match request.kind {
        UnitKind::Executable => request
            .output_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(|| project_root.join("target/incan").join(&name).join(request.profile))
            .join(&name),
        UnitKind::Library => artifact_dir(&project_root, request.profile).join(format!("lib{name}.rlib")),
    };
    if let Some(directory) = output.parent() {
        fs::create_dir_all(directory).map_err(|error| {
            CliError::failure(format!("cannot create the output directory {}: {error}", directory.display()))
        })?;
    }

    // ---- The pinned toolchain, the driver and the unit's native closure ----
    let rustc = resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
    let sysroot = rustc_sysroot(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let driver = rustc_driver()?;
    let runtime = native_runtime(request.profile)?;
    let mut externs = runtime.externs.clone();
    let mut search = runtime.directories.iter().cloned().collect::<BTreeSet<_>>();
    for entry in session.loaves().entries() {
        externs.push((crate_name(&entry.artifact.name), entry.artifact.rlib_path()));
        let closure = entry
            .artifact
            .native_closure()
            .map_err(|error| CliError::failure(error.to_string()))?;
        search.extend(closure.search);
        search.extend(
            closure
                .externs
                .iter()
                .filter_map(|(_, rlib)| rlib.parent().map(Path::to_path_buf)),
        );
    }

    // ---- Compile ----
    progress(format!("Compiling {} ({})", source.display(), request.profile));
    let mut command = Command::new(&driver);
    command
        .arg("--source")
        .arg(source)
        .arg(crate_name(&name))
        .arg(&output)
        .arg(&sysroot)
        .arg("--profile")
        .arg(request.profile)
        .arg("--package-name")
        .arg(&name);
    if request.kind == UnitKind::Library {
        command.arg("--library");
    }
    for (crate_name, artifact) in &externs {
        command.arg("--extern").arg(format!("{crate_name}={}", artifact.display()));
    }
    for directory in &search {
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
    Ok(UnitOutput {
        artifact: output,
        project_root,
        name,
    })
}

/// Compile every Loaf a unit reaches that has no artifact for `profile`: the standard packages in their policy order,
/// each reaching only the ones before it, then the unit's path dependencies, each after its own.
fn prepare_loaves(
    manifest: Option<&ProjectManifest>,
    profile: &str,
    progress: &mut impl FnMut(String),
) -> CliResult<()> {
    let own = manifest
        .and_then(|manifest| manifest.project.as_ref()?.name.clone())
        .unwrap_or_default();
    if let Some(std_root) = standard_root()? {
        for policy in STANDARD_PACKAGE_NAMESPACE_POLICIES {
            if policy.package_name == own {
                break;
            }
            let root = std_root.join(policy.source_directory);
            if root.join(LOAF_MANIFEST_FILENAME).is_file() && !has_artifact(&root, policy.package_name, profile) {
                compile_library(&root, profile, progress)?;
            }
        }
    }
    let Some(manifest) = manifest else {
        return Ok(());
    };
    let mut prepared = BTreeSet::new();
    for spec in manifest.library_dependencies().values() {
        prepare_dependency(&spec.path, profile, &mut prepared, progress)?;
    }
    Ok(())
}

/// Compile one path dependency after its own dependencies, unless it already has an artifact for `profile`.
fn prepare_dependency(
    root: &Path,
    profile: &str,
    prepared: &mut BTreeSet<PathBuf>,
    progress: &mut impl FnMut(String),
) -> CliResult<()> {
    if !prepared.insert(root.to_path_buf()) {
        return Ok(());
    }
    let manifest = ProjectManifest::load(&root.join(LOAF_MANIFEST_FILENAME))
        .map_err(|error| CliError::failure(format!("dependency at {}: {error}", root.display())))?;
    for spec in manifest.library_dependencies().values() {
        prepare_dependency(&spec.path, profile, prepared, progress)?;
    }
    let name = manifest
        .project
        .as_ref()
        .and_then(|project| project.name.clone())
        .ok_or_else(|| CliError::failure(format!("dependency at {} names no package", root.display())))?;
    if !has_artifact(root, &name, profile) {
        compile_library(root, profile, progress)?;
    }
    Ok(())
}

/// The entry source of the Loaf whose project contains `path`: its source root's `lib.incn`.
pub fn library_entry(path: &Path) -> CliResult<PathBuf> {
    let start = if path.is_dir() {
        path.to_path_buf()
    } else {
        resolve_project_root(path)
    };
    let manifest = discover_effective_project_manifest(&start)?.ok_or_else(|| {
        CliError::failure(format!("{} is not inside a Loaf project (no loaf.toml)", start.display()))
    })?;
    Ok(resolve_source_root(manifest.project_root(), Some(&manifest)).join("lib.incn"))
}

/// Compile the Loaf at `root` as a library unit.
fn compile_library(root: &Path, profile: &str, progress: &mut impl FnMut(String)) -> CliResult<UnitOutput> {
    let source = library_entry(root)?;
    let request = UnitRequest {
        source: &source,
        kind: UnitKind::Library,
        profile,
        features: &FeatureSelection::default(),
        output_dir: None,
    };
    compile_prepared_unit(&source, &request, progress)
}

/// Whether the Loaf `name` at `root` has a manifest for `profile`.
fn has_artifact(root: &Path, name: &str, profile: &str) -> bool {
    artifact_dir(root, profile).join(format!("{name}.incnlib")).is_file()
}

/// The unit's name: the manifest's project name when there is one, otherwise the source file's stem.
fn unit_name(manifest: Option<&ProjectManifest>, source: &Path) -> CliResult<String> {
    if let Some(name) = manifest
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

/// A Rust crate name for a unit: ASCII alphanumerics and `_`, never starting with a digit.
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

/// The toolchain's standard-library root: `ROOT/stdlib` installed, `ROOT/loaves/stdlib` in a checkout.
fn standard_root() -> CliResult<Option<PathBuf>> {
    let Some(layout) = CompilerOwnedSourceLayout::discover().map_err(|error| CliError::failure(error.to_string()))?
    else {
        return Ok(None);
    };
    let root = layout
        .verified_source_root()
        .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(Some(root.to_path_buf()))
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

/// The externs and search paths every unit links, held with the store lease that keeps them in place.
struct NativeRuntime {
    externs: Vec<(String, PathBuf)>,
    directories: Vec<PathBuf>,
    /// The selected plan owns the execution lease; the closure's paths stay valid while it is alive.
    _plan: OvenStoredDirectRustcExecutionPlan,
}

/// Select the prepared native runtime's stored plan for `profile` and add the runtime library itself.
fn native_runtime(profile: &str) -> CliResult<NativeRuntime> {
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
    Ok(NativeRuntime {
        externs,
        directories: plan.artifact_plan.dependency_search_paths.clone(),
        _plan: plan,
    })
}
