//! RFC 023: Stdlib module loader for build pipeline.
//!
//! This module handles automatic detection and loading of stdlib `.incn` files when they're imported in user code.
//! Unlike user modules (which are discovered via filesystem traversal in `collect_modules`), stdlib modules are:
//!
//! 1. Detected by scanning import statements for `std.*` paths
//! 2. Resolved via `incan_core::lang::stdlib::stdlib_stub_path()`
//! 3. Parsed and added as dependency modules to the codegen pipeline
//!
//! ## Integration
//!
//! Call `load_stdlib_modules()` after `collect_modules()` and before passing modules to `IrCodegen`:
//!
//! ```rust,ignore
//! let user_modules = collect_modules(file_path)?;
//! let stdlib_modules = load_stdlib_modules(&user_modules)?;
//! // Pass both to codegen...
//! ```

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use crate::cli::{CliError, CliResult};
use crate::frontend::{ast_walk, diagnostics, lexer::Lexer, parser::Parser};
use incan_core::lang::stdlib;
use incan_syntax::ast::{Declaration, ImportKind, Program};

/// A parsed stdlib module ready for compilation.
#[derive(Debug)]
pub struct StdlibModule {
    /// Module name (flat, e.g., "std_testing")
    pub name: String,
    /// Path segments (e.g., ["std", "testing"])
    pub path_segments: Vec<String>,
    /// Filesystem path where the .incn file was found
    pub file_path: PathBuf,
    /// Source code
    pub source: String,
    /// Parsed AST
    pub ast: Program,
}

/// Detect and load all stdlib modules imported by the given modules.
///
/// Scans all imports in `modules` for `std.*` paths, resolves them to stdlib `.incn` files, parses them,
/// and returns them as `StdlibModule` entries.
///
/// ## Errors
///
/// Returns an error if:
/// - A stdlib module file cannot be found
/// - A stdlib module file cannot be read
/// - A stdlib module fails to parse
pub fn load_stdlib_modules(modules: &[crate::cli::prelude::ParsedModule]) -> CliResult<Vec<StdlibModule>> {
    let mut stdlib_paths: HashSet<Vec<String>> = HashSet::new();

    // ---- Collect all stdlib imports ----
    for module in modules {
        if uses_iterator_adapter_surface(&module.ast) {
            stdlib_paths.insert(vec!["std".to_string(), "derives".to_string(), "collection".to_string()]);
        }
        for import in collect_imports(&module.ast) {
            let path = match &import.kind {
                ImportKind::From { module, .. } => module.segments.clone(),
                ImportKind::Module(p) => p.segments.clone(),
                // Skip external namespace imports.
                ImportKind::RustCrate { .. }
                | ImportKind::RustFrom { .. }
                | ImportKind::PubLibrary { .. }
                | ImportKind::PubFrom { .. }
                | ImportKind::Python(_) => continue,
            };

            if is_stdlib_import(&path) {
                stdlib_paths.insert(path);
            }
        }
    }

    // ---- Load and parse each stdlib module ----
    let mut stdlib_modules = Vec::new();
    for path in stdlib_paths {
        let module = load_stdlib_module(&path)?;
        stdlib_modules.push(module);
    }

    Ok(stdlib_modules)
}

/// Return whether stdlib collection should be loaded for RFC 088 iterator surface methods.
fn uses_iterator_adapter_surface(program: &Program) -> bool {
    ast_walk::any_expr_in_program(program, |expr| match expr {
        incan_syntax::ast::Expr::MethodCall(_, method, _, _) => matches!(
            method.as_str(),
            "iter"
                | "map"
                | "filter"
                | "enumerate"
                | "zip"
                | "take"
                | "skip"
                | "take_while"
                | "skip_while"
                | "chain"
                | "flat_map"
                | "batch"
                | "collect"
                | "count"
                | "reduce"
                | "fold"
                | "any"
                | "all"
                | "find"
                | "for_each"
                | "sum"
        ),
        _ => false,
    })
}

/// Check if an import path refers to a stdlib module (starts with "std").
fn is_stdlib_import(path: &[String]) -> bool {
    path.first().is_some_and(|s| s == "std")
}

/// Load and parse a single stdlib module.
fn load_stdlib_module(path: &[String]) -> CliResult<StdlibModule> {
    // Resolve stdlib path via incan_core registry
    let relative_path = stdlib::stdlib_stub_path(path).ok_or_else(|| {
        CliError::failure(format!(
            "Stdlib module not found: {} (not in stdlib registry)",
            path.join("::")
        ))
    })?;

    // Find the absolute path to the stdlib file
    let abs_path = find_stdlib_file(&relative_path)?;

    // Read source
    let source = fs::read_to_string(&abs_path)
        .map_err(|e| CliError::failure(format!("Failed to read stdlib module {}: {}", path.join("::"), e)))?;

    // Parse
    let tokens = Lexer::new(&source).tokenize().map_err(|errs| {
        let mut msg = String::new();
        for err in &errs {
            msg.push_str(&diagnostics::format_error(
                &format!("stdlib/{}", relative_path),
                &source,
                err,
            ));
        }
        CliError::failure(msg.trim_end())
    })?;

    let ast = Parser::new(&tokens).parse().map_err(|errs| {
        let mut msg = String::new();
        for err in &errs {
            msg.push_str(&diagnostics::format_error(
                &format!("stdlib/{}", relative_path),
                &source,
                err,
            ));
        }
        CliError::failure(msg.trim_end())
    })?;

    Ok(StdlibModule {
        name: path.join("_"),
        path_segments: path.to_vec(),
        file_path: abs_path,
        source,
        ast,
    })
}

/// Find the absolute path to a stdlib `.incn` file.
///
/// Searches in the following locations (in order):
/// 1. `$INCAN_STDLIB_DIR/<path>` (explicit override, runtime env var)
/// 2. Workspace crate (compile-time): `$CARGO_MANIFEST_DIR/crates/incan_stdlib/<path>`
/// 3. CWD crate-relative: `crates/incan_stdlib/<path>`
/// 4. CWD relative: `<path>`
/// 5. Installed stdlib (runtime env var): `$INCAN_STDLIB_PATH/<path>`
///
/// Returns an error if the file cannot be found in any location.
fn find_stdlib_file(relative_path: &str) -> CliResult<PathBuf> {
    // 1. Explicit override root (runtime)
    if let Ok(dir) = std::env::var("INCAN_STDLIB_DIR") {
        let p = PathBuf::from(dir).join(relative_path);
        if p.exists() {
            return Ok(p);
        }
    }

    // 2. Development build: workspace-relative (compile-time path)
    // CARGO_MANIFEST_DIR is the workspace root (where `incan` crate's Cargo.toml lives)
    let workspace_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/incan_stdlib")
        .join(relative_path);
    if workspace_path.exists() {
        return Ok(workspace_path);
    }

    // 3-4. Relative to current working directory
    if let Ok(cwd) = std::env::current_dir() {
        let crate_local = cwd.join("crates/incan_stdlib").join(relative_path);
        if crate_local.exists() {
            return Ok(crate_local);
        }
        let local = cwd.join(relative_path);
        if local.exists() {
            return Ok(local);
        }
    }

    // 5. Installed stdlib path (runtime, for production installs)
    if let Ok(stdlib_root) = std::env::var("INCAN_STDLIB_PATH") {
        let installed_path = PathBuf::from(stdlib_root).join(relative_path);
        if installed_path.exists() {
            return Ok(installed_path);
        }
    }

    Err(CliError::failure(format!(
        "Stdlib file not found: {} (searched INCAN_STDLIB_DIR, workspace, cwd, and INCAN_STDLIB_PATH)",
        relative_path
    )))
}

/// Extract all top-level import statements from a program.
fn collect_imports(program: &Program) -> Vec<&incan_syntax::ast::ImportDecl> {
    program
        .declarations
        .iter()
        .filter_map(|decl| match &decl.node {
            Declaration::Import(import) => Some(import),
            _ => None,
        })
        .collect()
}
