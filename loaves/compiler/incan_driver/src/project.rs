//! Where a command's project is: manifest discovery, project and source roots, output-directory validation, and
//! bounded source reading.
//!
//! These answer questions every command asks before it compiles anything, so they live below the session rather
//! than inside it.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use incan_core::lang::stdlib;

use crate::diagnostics::CliDiagnosticFailure;
use crate::error::{CliError, CliResult};
use incan_frontend::ast::Span;
use incan_frontend::diagnostics;
use oven_model::manifest::{
    CARGO_MANIFEST_FILENAME, DiscoveredManifest, LOAF_MANIFEST_FILENAME, ManifestError, ProjectManifest,
    discovered_manifest_kind,
};
use oven_model::workspace::WorkspaceGraph;
/// Maximum source file size (100 MB)
///
/// Files larger than this are rejected to prevent out-of-memory conditions during compilation.
const MAX_SOURCE_SIZE: u64 = 100 * 1024 * 1024;

/// Project roots already told that their `Cargo.toml` is ignored, so one command warns once.
///
/// Oven prepares a project more than once per command — once per selected profile, and again for a caller-owned
/// library graph — so emitting at the preparation boundary without this would repeat the same line several times for
/// a single `incan build`. Keyed by project root rather than a global flag, so a workspace build still reports each
/// member that has one.
static IGNORED_CARGO_MANIFESTS_REPORTED: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

#[derive(Debug, Clone)]
struct SourceReadFailure {
    message: String,
}

/// Discover a project manifest and materialize explicit RFC 077 inheritance before compiler stages consume it.
///
/// This is the single-project compatibility boundary: projects outside a workspace retain their parsed manifest, while
/// a member receives the graph-owned effective manifest. A dangling `{ workspace = true }` request is always an error
/// instead of being silently treated as an absent local dependency.
pub fn discover_effective_project_manifest(start_dir: &Path) -> CliResult<Option<ProjectManifest>> {
    let Some(manifest) = ProjectManifest::discover(start_dir).map_err(|error| CliError::failure(error.to_string()))?
    else {
        return Ok(None);
    };
    effective_project_manifest(manifest).map(Some)
}

/// Load one exact project root and materialize RFC 077 inheritance without applying command-discovery overrides.
///
/// Recursive source-authority traversal already owns the exact dependency root it is inspecting. Rediscovering from
/// that root could honor a nested command override and silently bind a different project, so this boundary loads the
/// named manifest directly while sharing the same workspace resolution as ordinary command discovery.
pub fn effective_project_manifest_for_exact_root(project_root: &Path) -> CliResult<ProjectManifest> {
    let manifest = ProjectManifest::load(&project_root.join(LOAF_MANIFEST_FILENAME))
        .map_err(|error| CliError::failure(error.to_string()))?;
    effective_project_manifest(manifest)
}

/// Resolve one parsed manifest against its active RFC 077 workspace, if any.
fn effective_project_manifest(manifest: ProjectManifest) -> CliResult<ProjectManifest> {
    let workspace =
        WorkspaceGraph::discover(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?;
    let Some(workspace) = workspace else {
        if manifest.has_workspace_inherited_dependencies() {
            return Err(CliError::failure(format!(
                "{} declares {{ workspace = true }} dependencies but is not a member of an active workspace",
                manifest.path().display()
            )));
        }
        return Ok(manifest);
    };
    let canonical_root = std::fs::canonicalize(manifest.project_root()).map_err(|error| {
        CliError::failure(format!(
            "failed to canonicalize project root {}: {error}",
            manifest.project_root().display()
        ))
    })?;
    let member = workspace.member_for_root(&canonical_root).ok_or_else(|| {
        CliError::failure(format!(
            "project {} is not a member of the active workspace at {}",
            manifest.path().display(),
            workspace.root().display()
        ))
    })?;
    workspace
        .effective_member_manifest(member)
        .map_err(|error| CliError::failure(error.to_string()))
}

/// Resolve the source path for a stdlib module path (e.g. `["std", "testing"]`).
pub fn resolve_stdlib_module_source_path(module_path: &[String]) -> CliResult<PathBuf> {
    let Some(relative_stub_path) = stdlib::stdlib_stub_path(module_path) else {
        return Err(CliError::failure(format!(
            "Cannot resolve source for non-stdlib module path '{}'.",
            module_path.join(".")
        )));
    };

    if let Some(path) = incan_frontend::provider::find_stdlib_source_file(&relative_stub_path) {
        return Ok(path);
    }

    Err(CliError::failure(format!(
        "Cannot resolve source file for '{}'; expected '{}' under stdlib search roots.",
        module_path.join("."),
        relative_stub_path
    )))
}

/// Read source file contents.
///
/// ## Errors
///
/// Returns an error if:
/// - The file cannot be read (I/O error)
/// - The file exceeds `MAX_SOURCE_SIZE` (100 MB)
pub fn read_source(file_path: &str) -> CliResult<String> {
    read_source_checked(file_path).map_err(|failure| CliError::failure(failure.message))
}

/// Read source for the stable diagnostic path, converting file-system failures into tooling diagnostics.
pub fn read_source_for_diagnostics(file_path: &str) -> Result<String, CliDiagnosticFailure> {
    read_source_checked(file_path).map_err(|failure| {
        CliDiagnosticFailure::single(
            file_path,
            "",
            diagnostics::CompileError::new(failure.message, Span::default()),
            diagnostics::DiagnosticPhase::Tooling,
        )
    })
}

/// Read source once behind both legacy text errors and structured diagnostic reporting.
fn read_source_checked(file_path: &str) -> Result<String, SourceReadFailure> {
    let metadata = fs::metadata(file_path).map_err(|error| SourceReadFailure {
        message: format!("Cannot access file '{}': {}", file_path, error),
    })?;
    if metadata.len() > MAX_SOURCE_SIZE {
        return Err(SourceReadFailure {
            message: format!(
                "Source file '{}' is too large ({} bytes, max {} bytes)",
                file_path,
                metadata.len(),
                MAX_SOURCE_SIZE
            ),
        });
    }
    fs::read_to_string(file_path).map_err(|error| SourceReadFailure {
        message: format!("Error reading file '{}': {}", file_path, error),
    })
}

/// Recursively collect authored `.incn` files without following directory symlinks.
pub fn collect_incan_source_files(directory: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_str().unwrap_or("");
            // Broad source collection is used for package libraries and generated test batches. Compiler, editor,
            // and user tools all place non-source state below hidden directories; treating that state as an Incan
            // module can emit invalid Rust such as `pub mod .incan;`.
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                collect_incan_source_files(&entry.path(), files)?;
            }
        } else if file_type.is_file() && entry.path().extension().is_some_and(|extension| extension == "incn") {
            files.push(entry.path());
        }
    }
    Ok(())
}

/// Report an ignored `Cargo.toml` beside a Loaf manifest, at most once per project root per invocation.
///
/// RFC 117 rule 11: a `loaf.toml` project containing `Cargo.toml` must warn and ignore the Cargo configuration, and
/// the diagnostic must name the ignored file and explain that Cargo compatibility is selected explicitly. Both are
/// carried by [`ManifestError::CargoIgnored`], which renders the text; this decides only when it reaches the user.
///
/// It warns rather than fails, and never inspects the Cargo file: the RFC requires Oven to "continue as a Loaf
/// project" and say what it ignored, and reading the file to describe it better would be the parsing rule 11
/// forbids. A directory that holds no `loaf.toml` is silent here — a Cargo-only project is Cargo-compatibility
/// mode's subject, not an ignored file.
///
/// Callers are the project-scale command entry points rather than one deep shared helper, because Oven's cached
/// paths return a completed output without preparing the project at all; a warning behind preparation would appear
/// on a cold build and vanish on a warm one, which is worse than not having it.
pub fn warn_once_about_ignored_cargo_manifest(project_root: &Path) {
    let DiscoveredManifest::Loaf(_) = discovered_manifest_kind(project_root) else {
        return;
    };
    let cargo_manifest = project_root.join(CARGO_MANIFEST_FILENAME);
    if !cargo_manifest.is_file() {
        return;
    }
    let key = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let Ok(mut reported) = IGNORED_CARGO_MANIFESTS_REPORTED.lock() else {
        // A poisoned registry means another thread panicked mid-report. Losing the deduplication is the right
        // failure here: a repeated warning is noise, a dropped one hides that Cargo configuration was ignored.
        eprintln!("warning: {}", ManifestError::CargoIgnored { path: cargo_manifest });
        return;
    };
    if reported.insert(key) {
        eprintln!("warning: {}", ManifestError::CargoIgnored { path: cargo_manifest });
    }
}

/// Resolve the project root from a source file path.
///
/// If the file is inside a `src/` directory (e.g. `src/main.incn` or `projects/foo/src/main.incn`), the project root
/// is the parent of `src/`. Otherwise, the project root is the file's parent directory.
///
/// Returns `"."` when the computed root would be empty (which happens for relative paths like `src/main.incn` where
/// the parent of `"src"` is `""`).
pub fn resolve_project_root(file_path: &Path) -> PathBuf {
    file_path
        .parent()
        .and_then(|p| {
            if p.file_name().is_some_and(|name| name == "src") {
                p.parent()
            } else {
                Some(p)
            }
        })
        .map(|p| {
            if p.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                p.to_path_buf()
            }
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Resolve the source root directory for a project.
///
/// The source root is where user module imports are resolved from. Resolution order:
///
/// 1. Explicit `[build] source-root` in the manifest (e.g. `source-root = "lib"`)
/// 2. Convention: `src/` directory exists relative to project root
/// 3. Fallback: project root itself (flat layout)
///
/// This is used by both the build pipeline and the test runner so that `from greet import greet` resolves to the same
/// file everywhere.
pub fn resolve_source_root(project_root: &Path, manifest: Option<&ProjectManifest>) -> PathBuf {
    // ---- Explicit configuration ----
    if let Some(source_root) = manifest
        .and_then(|m| m.build.as_ref())
        .and_then(|b| b.source_root.as_deref())
    {
        return project_root.join(source_root);
    }

    // ---- Convention: src/ directory ----
    let src_dir = project_root.join("src");
    if src_dir.is_dir() {
        return src_dir;
    }

    // ---- Fallback: project root (flat layout) ----
    project_root.to_path_buf()
}

/// Validate the output directory to prevent path traversal attacks.
///
/// This function ensures:
/// - The path doesn't contain `..` components
/// - The path doesn't start with `/` (absolute path outside workspace) unless it starts with a known safe prefix
pub fn validate_output_dir(out_dir: &str) -> CliResult<()> {
    let path = Path::new(out_dir);

    // Check for path traversal attempts
    for component in path.components() {
        if let std::path::Component::ParentDir = component {
            return Err(CliError::failure(format!(
                "Output directory '{}' contains path traversal (..)",
                out_dir
            )));
        }
    }

    // Warn about absolute paths (but allow them for flexibility)
    if path.is_absolute() {
        tracing::warn!(
            "Using absolute output path: {}. Consider using a relative path.",
            out_dir
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_root_from_relative_src_is_dot_not_empty() {
        // Regression: `src/main.incn` used to yield "" instead of ".", causing
        // `Command::current_dir("")` to fail with ENOENT.
        let root = resolve_project_root(Path::new("src/main.incn"));
        assert_eq!(root, PathBuf::from("."));
    }

    #[test]
    fn project_root_from_nested_src_path() {
        let root = resolve_project_root(Path::new("projects/greeter/src/main.incn"));
        assert_eq!(root, PathBuf::from("projects/greeter"));
    }

    #[test]
    fn project_root_from_absolute_src_path() {
        let root = resolve_project_root(Path::new("/home/user/project/src/main.incn"));
        assert_eq!(root, PathBuf::from("/home/user/project"));
    }

    #[test]
    fn project_root_when_file_is_not_in_src() {
        // File directly in a directory, not in src/
        let root = resolve_project_root(Path::new("main.incn"));
        assert_eq!(root, PathBuf::from("."));
    }

    #[test]
    fn project_root_from_non_src_subdirectory() {
        let root = resolve_project_root(Path::new("lib/utils.incn"));
        assert_eq!(root, PathBuf::from("lib"));
    }

    // ---- resolve_source_root ----

    #[test]
    fn source_root_uses_src_convention() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("myproject");
        fs::create_dir_all(project.join("src"))?;

        let root = resolve_source_root(&project, None);
        assert_eq!(root, project.join("src"));
        Ok(())
    }

    #[test]
    fn source_root_falls_back_to_project_root_when_no_src() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("flat_project");
        fs::create_dir_all(&project)?;

        let root = resolve_source_root(&project, None);
        assert_eq!(root, project);
        Ok(())
    }

    #[test]
    fn source_root_respects_explicit_manifest_config() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("custom_src");
        fs::create_dir_all(project.join("src"))?; // src/ exists but should be overridden

        let manifest_content = r#"
[build]
source-root = "lib"
"#;
        let manifest = ProjectManifest::from_str(manifest_content, &project.join("loaf.toml"))?;

        let root = resolve_source_root(&project, Some(&manifest));
        assert_eq!(root, project.join("lib"));
        Ok(())
    }

    #[test]
    fn broad_source_collection_ignores_hidden_and_generated_directories() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::write(tmp.path().join("root.incn"), "def root() -> None:\n  pass\n")?;
        fs::create_dir_all(tmp.path().join("nested"))?;
        fs::write(tmp.path().join("nested/module.incn"), "def nested() -> None:\n  pass\n")?;
        for directory in [".ralph-cache", ".incan", "target", "node_modules"] {
            let hidden = tmp.path().join(directory);
            fs::create_dir_all(&hidden)?;
            fs::write(hidden.join("not_source.incn"), "def ignored() -> None:\n  pass\n")?;
        }

        let mut files = Vec::new();
        collect_incan_source_files(tmp.path(), &mut files)?;
        files.sort();
        let relative = files
            .iter()
            .map(|path| path.strip_prefix(tmp.path()).map(Path::to_path_buf))
            .collect::<Result<Vec<_>, _>>()?;

        assert_eq!(
            relative,
            vec![PathBuf::from("nested/module.incn"), PathBuf::from("root.incn")]
        );
        Ok(())
    }
}
