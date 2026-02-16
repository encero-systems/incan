//! Shared utilities used across multiple CLI command pipelines.
//!
//! This module contains functions for source file reading, module collection, project root resolution,
//! dependency helpers, and Cargo flag construction.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::prelude::ParsedModule;
use crate::cli::{CliError, CliResult};
use crate::dependency_resolver::{DependencyError, InlineRustImport};
use crate::frontend::ast::ImportKind;
use crate::frontend::{diagnostics, lexer, parser};
use crate::lockfile::CargoFeatureSelection;
use crate::manifest::ProjectManifest;
use incan_core::lang::stdlib;

/// Maximum source file size (100 MB)
///
/// Files larger than this are rejected to prevent out-of-memory conditions during compilation.
const MAX_SOURCE_SIZE: u64 = 100 * 1024 * 1024;

/// Resolve the source path for a stdlib module path (e.g. `["std", "testing"]`).
pub(crate) fn resolve_stdlib_module_source_path(module_path: &[String]) -> CliResult<PathBuf> {
    let Some(relative_stub_path) = stdlib::stdlib_stub_path(module_path) else {
        return Err(CliError::failure(format!(
            "Cannot resolve source for non-stdlib module path '{}'.",
            module_path.join(".")
        )));
    };

    let stdlib_relative = relative_stub_path
        .strip_prefix("stdlib/")
        .unwrap_or(relative_stub_path.as_str());
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(stdlib_dir) = crate::cli::prelude::find_stdlib_dir() {
        candidates.push(stdlib_dir.join(stdlib_relative));
    }
    candidates.push(PathBuf::from(&relative_stub_path));
    candidates.push(PathBuf::from("crates/incan_stdlib").join(&relative_stub_path));

    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
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
    // Check file size before reading
    let metadata =
        fs::metadata(file_path).map_err(|e| CliError::failure(format!("Cannot access file '{}': {}", file_path, e)))?;

    if metadata.len() > MAX_SOURCE_SIZE {
        return Err(CliError::failure(format!(
            "Source file '{}' is too large ({} bytes, max {} bytes)",
            file_path,
            metadata.len(),
            MAX_SOURCE_SIZE
        )));
    }

    fs::read_to_string(file_path).map_err(|e| CliError::failure(format!("Error reading file '{}': {}", file_path, e)))
}

/// Collect and parse the entry file and all its dependencies.
///
/// # Note on Prelude
///
/// The stdlib prelude (`stdlib/prelude.incn`) exists but is not currently wired into the compilation pipeline.
/// Prelude traits like `Debug`, `Display`, `Clone` are recognized by codegen heuristics rather than actual trait
/// definitions.
///
/// Future work: integrate prelude ASTs into typechecking so trait bounds are validated and derives work through actual
/// trait implementations.
pub fn collect_modules(entry_path: &str) -> CliResult<Vec<ParsedModule>> {
    let path = Path::new(entry_path);
    let base_dir = path.parent().unwrap_or(Path::new("."));

    let mut modules = Vec::new();
    let mut processed = HashSet::new();
    let mut incan_source_stdlib_module_paths: HashMap<String, PathBuf> = HashMap::new();
    // (file_path, module_name, path_segments)
    let mut to_process: Vec<(String, String, Vec<String>)> =
        vec![(entry_path.to_string(), "main".to_string(), vec!["main".to_string()])];

    while let Some((file_path, module_name, path_segments)) = to_process.pop() {
        if processed.contains(&file_path) {
            continue;
        }
        processed.insert(file_path.clone());

        let source = read_source(&file_path)?;
        let tokens = match lexer::lex(&source) {
            Ok(t) => t,
            Err(errs) => {
                let mut msg = String::new();
                for err in &errs {
                    msg.push_str(&diagnostics::format_error(&file_path, &source, err));
                    msg.push('\n');
                }
                return Err(CliError::failure(msg.trim_end()));
            }
        };

        let ast = match parser::parse(&tokens) {
            Ok(a) => a,
            Err(errs) => {
                let mut msg = String::new();
                for err in &errs {
                    msg.push_str(&diagnostics::format_error(&file_path, &source, err));
                    msg.push('\n');
                }
                return Err(CliError::failure(msg.trim_end()));
            }
        };

        // Find imports and add them to process queue
        for decl in &ast.declarations {
            if let crate::frontend::ast::Declaration::Import(import) = &decl.node {
                let import_path = match &import.kind {
                    crate::frontend::ast::ImportKind::Module(path) if !path.segments.is_empty() => Some(path),
                    crate::frontend::ast::ImportKind::From { module, .. } if !module.segments.is_empty() => {
                        Some(module)
                    }
                    _ => None,
                };

                if let Some(path) = import_path {
                    if path.segments.is_empty() {
                        continue;
                    }

                    if path.parent_levels == 0 && !path.is_absolute && stdlib::is_any_stdlib_path(&path.segments) {
                        if matches!(
                            stdlib::stdlib_impl_mode_for(&path.segments),
                            Some(stdlib::StdlibImplMode::IncanSource)
                        ) {
                            let stdlib_key = path.segments.join(".");
                            let source_path =
                                if let Some(cached_path) = incan_source_stdlib_module_paths.get(&stdlib_key) {
                                    cached_path.clone()
                                } else {
                                    let resolved = resolve_stdlib_module_source_path(&path.segments)?;
                                    incan_source_stdlib_module_paths.insert(stdlib_key, resolved.clone());
                                    resolved
                                };

                            let mut module_segments = vec![stdlib::INCAN_STD_NAMESPACE.to_string()];
                            module_segments.extend(path.segments.iter().skip(1).cloned());
                            let module_name = module_segments.join("_");
                            let dep_path_str = source_path.to_string_lossy().to_string();
                            if !processed.contains(&dep_path_str) {
                                to_process.push((dep_path_str, module_name, module_segments));
                            }
                        }
                        continue;
                    }

                    let mut target_dir = base_dir.to_path_buf();

                    if path.is_absolute {
                        let mut project_root = base_dir.to_path_buf();
                        while !project_root.join("Cargo.toml").exists() && !project_root.join("src").exists() {
                            if let Some(parent) = project_root.parent() {
                                project_root = parent.to_path_buf();
                            } else {
                                break;
                            }
                        }
                        if project_root.join("src").exists() {
                            target_dir = project_root.join("src");
                        } else {
                            target_dir = project_root;
                        }
                    } else {
                        for _ in 0..path.parent_levels {
                            target_dir = target_dir.parent().map(|p| p.to_path_buf()).unwrap_or(target_dir);
                        }
                    }

                    let module_segments = match &import.kind {
                        crate::frontend::ast::ImportKind::From { module, .. } => module.segments.clone(),
                        crate::frontend::ast::ImportKind::Module(p) => {
                            if p.segments.len() > 1 {
                                p.segments[..p.segments.len() - 1].to_vec()
                            } else {
                                p.segments.clone()
                            }
                        }
                        _ => continue,
                    };

                    if module_segments.is_empty() {
                        continue;
                    }

                    let mut dep_path = target_dir.clone();
                    for segment in &module_segments {
                        dep_path = dep_path.join(segment);
                    }

                    dep_path.set_extension("incn");
                    let mut found_path: Option<PathBuf> = None;

                    if dep_path.exists() {
                        found_path = Some(dep_path.clone());
                    } else {
                        dep_path.set_extension("incan");
                        if dep_path.exists() {
                            found_path = Some(dep_path.clone());
                        }
                    }

                    if let Some(path) = found_path {
                        let dep_path_str = path.to_string_lossy().to_string();
                        let module_name = module_segments.join("_");
                        if !processed.contains(&dep_path_str) {
                            to_process.push((dep_path_str, module_name, module_segments.clone()));
                        }
                    }
                }
            }
        }

        modules.push(ParsedModule {
            name: module_name,
            path_segments,
            file_path: PathBuf::from(&file_path),
            source,
            ast,
        });
    }

    modules.reverse();
    Ok(modules)
}

/// Resolve the project root from a source file path.
///
/// If the file is inside a `src/` directory (e.g. `src/main.incn` or `projects/foo/src/main.incn`), the project root
/// is the parent of `src/`. Otherwise, the project root is the file's parent directory.
///
/// Returns `"."` when the computed root would be empty (which happens for relative paths like `src/main.incn` where
/// the parent of `"src"` is `""`).
pub(crate) fn resolve_project_root(file_path: &Path) -> PathBuf {
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
pub(crate) fn resolve_source_root(project_root: &Path, manifest: Option<&ProjectManifest>) -> PathBuf {
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
pub(crate) fn validate_output_dir(out_dir: &str) -> CliResult<()> {
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

/// Extract inline Rust crate imports from a parsed module.
pub(crate) fn collect_inline_rust_imports(module: &ParsedModule, is_test_context: bool) -> Vec<InlineRustImport> {
    let mut imports = Vec::new();

    for decl in &module.ast.declarations {
        let crate::frontend::ast::Declaration::Import(import) = &decl.node else {
            continue;
        };

        match &import.kind {
            ImportKind::RustCrate {
                crate_name,
                version,
                features,
                ..
            } => {
                imports.push(InlineRustImport {
                    crate_name: crate_name.clone(),
                    version: version.clone(),
                    features: features.clone(),
                    span: decl.span,
                    file_path: module.file_path.clone(),
                    is_test_context,
                });
            }
            ImportKind::RustFrom {
                crate_name,
                version,
                features,
                ..
            } => {
                imports.push(InlineRustImport {
                    crate_name: crate_name.clone(),
                    version: version.clone(),
                    features: features.clone(),
                    span: decl.span,
                    file_path: module.file_path.clone(),
                    is_test_context,
                });
            }
            _ => {}
        }
    }

    imports
}

/// Build a map of file paths to source contents for error reporting.
pub(crate) fn build_source_map(modules: &[ParsedModule]) -> HashMap<PathBuf, String> {
    let mut sources = HashMap::new();
    for module in modules {
        sources.insert(module.file_path.clone(), module.source.clone());
    }
    sources
}

/// Format a dependency resolution error with source-file context.
pub(crate) fn format_dependency_error(error: &DependencyError, sources: &HashMap<PathBuf, String>) -> String {
    let file_path = error.file_path.to_string_lossy();
    if let Some(source) = sources.get(&error.file_path) {
        return diagnostics::format_error(&file_path, source, &error.error);
    }
    if let Ok(source) = fs::read_to_string(&error.file_path) {
        return diagnostics::format_error(&file_path, &source, &error.error);
    }

    format!("error: {}\n  --> {}\n", error.error.message, error.file_path.display())
}

/// Build Cargo policy flags (`--locked` / `--frozen`).
pub(crate) fn cargo_policy_flags(locked: bool, frozen: bool) -> Vec<String> {
    if frozen {
        vec!["--frozen".to_string()]
    } else if locked {
        vec!["--locked".to_string()]
    } else {
        Vec::new()
    }
}

/// Build Cargo command flags (policy flags + feature flags).
pub(crate) fn cargo_command_flags(locked: bool, frozen: bool, cargo_features: &CargoFeatureSelection) -> Vec<String> {
    let mut flags = cargo_policy_flags(locked, frozen);
    if cargo_features.cargo_all_features {
        flags.push("--all-features".to_string());
    }
    if cargo_features.cargo_no_default_features {
        flags.push("--no-default-features".to_string());
    }
    if !cargo_features.cargo_features.is_empty() {
        flags.push("--features".to_string());
        flags.push(cargo_features.cargo_features.join(","));
    }
    flags
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // ---- resolve_project_root ----

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
        let manifest = ProjectManifest::from_str(manifest_content, &project.join("incan.toml"))?;

        let root = resolve_source_root(&project, Some(&manifest));
        assert_eq!(root, project.join("lib"));
        Ok(())
    }
}
