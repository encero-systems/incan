//! Lock file generation and resolution for Incan projects.
//!
//! Handles creating and validating `incan.lock` files that pin dependency versions for reproducible builds.
//! Used by both `incan lock` and the build pipeline.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::backend::ProjectGenerator;
use crate::cli::{CliError, CliResult, ExitCode};
use crate::dependency_resolver::{InlineRustImport, ResolvedDependencies, resolve_dependencies};
use crate::frontend::ast::ImportKind;
use crate::frontend::{diagnostics, lexer, parser};
use crate::lockfile::{CargoFeatureSelection, IncanLock, compute_deps_fingerprint};
use crate::manifest::ProjectManifest;

use super::common::{
    build_source_map, cargo_command_flags, collect_inline_rust_imports, collect_modules, format_dependency_error,
};

/// Generate or update incan.lock for a project.
pub fn lock_project(
    entry_file: Option<&PathBuf>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
) -> CliResult<ExitCode> {
    let start_dir = entry_file
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let manifest = ProjectManifest::discover(&start_dir)
        .map_err(|e| CliError::failure(e.to_string()))?
        .ok_or_else(|| CliError::failure("No incan.toml found (run `incan init`)"))?;

    let entry_path = if let Some(file) = entry_file {
        file.to_path_buf()
    } else if let Some(project) = &manifest.project {
        if let Some(main) = project.scripts.get("main") {
            manifest.project_root().join(main)
        } else {
            return Err(CliError::failure(
                "incan lock requires a FILE argument or [project.scripts].main",
            ));
        }
    } else {
        return Err(CliError::failure(
            "incan lock requires a FILE argument or [project.scripts].main",
        ));
    };

    let modules = collect_modules(&entry_path.to_string_lossy())?;
    let mut inline_imports = Vec::new();
    for module in &modules {
        inline_imports.extend(collect_inline_rust_imports(module, false));
    }

    inline_imports.extend(collect_test_inline_imports(manifest.project_root())?);

    let cargo_features = CargoFeatureSelection {
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();

    let resolved = resolve_dependencies(Some(&manifest), &inline_imports, true, &cargo_features).map_err(|errors| {
        let mut msg = String::new();
        let sources = build_source_map(&modules);
        for err in errors {
            msg.push_str(&format_dependency_error(&err, &sources));
        }
        CliError::failure(msg.trim_end())
    })?;

    let project_name = manifest
        .project
        .as_ref()
        .and_then(|p| p.name.clone())
        .or_else(|| entry_path.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| "incan_project".to_string());
    let rust_edition = manifest.build.as_ref().and_then(|b| b.rust_edition.clone());
    generate_lockfile(
        manifest.project_root(),
        &project_name,
        rust_edition,
        &resolved,
        &cargo_features,
    )?;

    Ok(ExitCode::SUCCESS)
}

/// Resolve the lock payload for a project build.
///
/// Returns `None` if no manifest is present (standalone file compilation).
/// Otherwise, loads or regenerates the lock file and returns the Cargo.lock payload.
pub(crate) fn resolve_lock_payload(
    project_root: &Path,
    project_name: &str,
    manifest: Option<&ProjectManifest>,
    resolved: &ResolvedDependencies,
    cargo_features: &CargoFeatureSelection,
    locked: bool,
    frozen: bool,
) -> CliResult<Option<String>> {
    if manifest.is_none() {
        return Ok(None);
    }

    let lock_path = project_root.join("incan.lock");
    let rust_edition = manifest.and_then(|m| m.build.as_ref().and_then(|b| b.rust_edition.clone()));
    let fingerprint = compute_deps_fingerprint(
        &resolved.dependencies,
        &resolved.dev_dependencies,
        cargo_features,
        Some(project_root),
    );

    let strict = locked || frozen;
    if strict {
        if let Some(message) = strict_git_source_error(resolved) {
            return Err(CliError::failure(message));
        }
    }
    if lock_path.exists() {
        let lock = IncanLock::load(&lock_path).map_err(|e| CliError::failure(e.to_string()))?;
        if lock.deps_fingerprint != fingerprint {
            if strict {
                return Err(CliError::failure(format!(
                    "incan.lock is out of date\n\n\
                     \x20 expected deps-fingerprint: {fingerprint}\n\
                     \x20   actual deps-fingerprint: {actual}\n\n\
                     This usually means your dependency inputs changed since the lock was generated:\n\n\
                     \x20 - incan.toml dependency entries changed, and/or\n\
                     \x20 - inline rust::... annotations changed, and/or\n\
                     \x20 - toolchain known-good defaults changed (if you rely on defaults)\n\
                     \x20 - Cargo feature selection changed\n\n\
                     Fix:\n\n\
                     \x20   incan lock\n\n\
                     Tip: Pin crate versions/features explicitly in incan.toml for stability \
                     across toolchain upgrades.",
                    actual = lock.deps_fingerprint,
                )));
            }
            let lock = generate_lockfile(
                project_root,
                project_name,
                rust_edition.clone(),
                resolved,
                cargo_features,
            )?;
            return Ok(Some(lock.cargo_lock_payload));
        }
        return Ok(Some(lock.cargo_lock_payload));
    }

    if strict {
        return Err(CliError::failure("incan.lock is missing; run `incan lock`".to_string()));
    }

    let lock = generate_lockfile(project_root, project_name, rust_edition, resolved, cargo_features)?;
    Ok(Some(lock.cargo_lock_payload))
}

/// Generate an `incan.lock` file by creating a temporary Cargo project and resolving dependencies.
pub(crate) fn generate_lockfile(
    project_root: &Path,
    project_name: &str,
    rust_edition: Option<String>,
    resolved: &ResolvedDependencies,
    cargo_features: &CargoFeatureSelection,
) -> CliResult<IncanLock> {
    let lock_dir = project_root.join("target").join("incan_lock");
    let mut generator = ProjectGenerator::new(&lock_dir, project_name, true);
    generator.set_dependencies(resolved.dependencies.clone());
    generator.set_dev_dependencies(resolved.dev_dependencies.clone());
    generator.set_include_dev_dependencies(true);
    generator.set_rust_edition(rust_edition);

    let rust_code = "fn main() {}";
    generator
        .generate(rust_code)
        .map_err(|e| CliError::failure(format!("Failed to generate lock project: {}", e)))?;

    let mut command = Command::new("cargo");
    command.arg("generate-lockfile");
    for flag in cargo_command_flags(false, false, cargo_features) {
        command.arg(flag);
    }
    let status = command
        .current_dir(&lock_dir)
        .output()
        .map_err(|e| CliError::failure(format!("Failed to run cargo generate-lockfile: {}", e)))?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        return Err(CliError::failure(format!(
            "cargo generate-lockfile failed:\n{}",
            stderr
        )));
    }

    let cargo_lock = fs::read_to_string(lock_dir.join("Cargo.lock"))
        .map_err(|e| CliError::failure(format!("Failed to read Cargo.lock: {}", e)))?;
    let fingerprint = compute_deps_fingerprint(
        &resolved.dependencies,
        &resolved.dev_dependencies,
        cargo_features,
        Some(project_root),
    );
    let lock = IncanLock::new(fingerprint, cargo_features.clone(), cargo_lock);

    let lock_path = project_root.join("incan.lock");
    lock.write(&lock_path)
        .map_err(|e| CliError::failure(format!("Failed to write incan.lock: {}", e)))?;

    Ok(lock)
}

/// Collect inline Rust crate imports from test files for lock resolution.
fn collect_test_inline_imports(project_root: &Path) -> CliResult<Vec<InlineRustImport>> {
    let mut imports = Vec::new();
    let test_files = crate::cli::test_runner::discover_test_files(project_root);

    for file_path in test_files {
        let source = fs::read_to_string(&file_path)
            .map_err(|e| CliError::failure(format!("Failed to read test file '{}': {}", file_path.display(), e)))?;
        let tokens = lexer::lex(&source).map_err(|errs| {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(&file_path.to_string_lossy(), &source, err));
            }
            CliError::failure(msg.trim_end())
        })?;
        let ast = parser::parse(&tokens).map_err(|errs| {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(&file_path.to_string_lossy(), &source, err));
            }
            CliError::failure(msg.trim_end())
        })?;

        for decl in &ast.declarations {
            let crate::frontend::ast::Declaration::Import(import) = &decl.node else {
                continue;
            };
            match &import.kind {
                ImportKind::RustCrate {
                    crate_name,
                    version,
                    features,
                    ..
                }
                | ImportKind::RustFrom {
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
                        file_path: file_path.clone(),
                        is_test_context: true,
                    });
                }
                _ => {}
            }
        }
    }

    Ok(imports)
}

/// Check whether any resolved dependency uses a git branch source, which is forbidden in strict
/// (`--locked` / `--frozen`) mode.
fn strict_git_source_error(resolved: &ResolvedDependencies) -> Option<String> {
    for spec in resolved.dependencies.iter().chain(resolved.dev_dependencies.iter()) {
        if let crate::manifest::DependencySource::Git { reference, .. } = &spec.source {
            if matches!(reference, crate::manifest::GitReference::Branch(_)) {
                return Some(format!(
                    "strict mode forbids git branch dependencies (crate `{}`); use tag or rev",
                    spec.crate_name
                ));
            }
        }
    }
    None
}
