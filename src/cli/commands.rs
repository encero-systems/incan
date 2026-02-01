//! CLI command implementations
//!
//! All command functions return `CliResult<ExitCode>` instead of calling
//! `process::exit`. Error handling and exits happen in the top-level `run()`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::backend::{IrCodegen, ProjectGenerator};
use crate::format::{format_diff, format_source};
use crate::frontend::ast::Program;
use crate::frontend::{diagnostics, lexer, parser, typechecker};
use incan_core::lang::stdlib;

use super::prelude::ParsedModule;
use super::{CliError, CliResult, ExitCode};

// ============================================================================
// Project Preparation (shared between build and run)
// ============================================================================

/// A prepared Incan project ready to be built or run.
///
/// This struct encapsulates all the setup work shared between `build_file()`
/// and `run_file()`, including module collection, type checking, codegen setup,
/// and project generation.
struct PreparedProject {
    /// The configured project generator
    generator: ProjectGenerator,
    /// Output directory path
    out_dir: String,
    /// Project root directory (used as working dir when running)
    project_root: PathBuf,
}

/// Prepare an Incan project for building or running.
///
/// This function performs all the shared setup:
/// 1. Collect and parse modules
/// 2. Type check
/// 3. Configure codegen (serde, async, web, etc.)
/// 4. Add Rust crate dependencies
/// 5. Generate Rust project files
fn prepare_project(file_path: &str, output_dir: Option<&str>) -> CliResult<PreparedProject> {
    let modules = collect_modules(file_path)?;

    let Some(main_module) = modules.last() else {
        return Err(CliError::failure("No modules found"));
    };

    let dep_modules = &modules[..modules.len() - 1];
    let deps: Vec<(&str, &Program)> = dep_modules.iter().map(|m| (m.name.as_str(), &m.ast)).collect();

    // Type check
    let mut checker = typechecker::TypeChecker::new();
    if let Err(errs) = checker.check_with_imports(&main_module.ast, &deps) {
        let mut msg = String::new();
        for err in &errs {
            msg.push_str(&diagnostics::format_error(file_path, &main_module.source, err));
        }
        return Err(CliError::failure(msg.trim_end()));
    }

    // Derive project name from file path
    let path = Path::new(file_path);
    let project_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("incan_project");
    let project_root = path
        .parent()
        .and_then(|p| {
            if p.file_name().is_some_and(|name| name == "src") {
                p.parent()
            } else {
                Some(p)
            }
        })
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let out_dir = output_dir
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("target/incan/{}", project_name));

    // Validate output directory path to prevent path traversal
    validate_output_dir(&out_dir)?;

    // Setup codegen
    let mut codegen = IrCodegen::new();
    for module in dep_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }
    codegen.scan_for_serde(&main_module.ast);
    codegen.scan_for_async(&main_module.ast);
    codegen.scan_for_web(&main_module.ast);
    codegen.scan_for_list_helpers(&main_module.ast);
    for module in dep_modules {
        codegen.scan_for_serde(&module.ast);
        codegen.scan_for_async(&module.ast);
        codegen.scan_for_web(&module.ast);
        codegen.scan_for_list_helpers(&module.ast);
    }

    let needs_serde = codegen.needs_serde();
    let needs_tokio = codegen.needs_tokio();
    let needs_axum = codegen.needs_axum();
    let mut rust_crates = collect_rust_crates(&main_module.ast);
    for module in dep_modules {
        for crate_name in collect_rust_crates(&module.ast) {
            if !rust_crates.contains(&crate_name) {
                rust_crates.push(crate_name);
            }
        }
    }

    // Setup project generator
    let mut generator = ProjectGenerator::new(&out_dir, project_name, true);
    generator.set_needs_serde(needs_serde);
    generator.set_needs_tokio(needs_tokio);
    generator.set_needs_axum(needs_axum);

    for crate_name in &rust_crates {
        generator.add_rust_crate(crate_name);
    }

    // Generate Rust project files
    let has_deps = !dep_modules.is_empty();
    if has_deps {
        let module_paths: Vec<Vec<String>> = dep_modules.iter().map(|m| m.path_segments.clone()).collect();
        let (main_code, rust_modules) = codegen
            .try_generate_multi_file_nested(&main_module.ast, &module_paths)
            .map_err(|e| CliError::failure(format!("Code generation error: {}", e)))?;

        generator
            .generate_nested(&main_code, &rust_modules)
            .map_err(|e| CliError::failure(format!("Error generating project: {}", e)))?;
    } else {
        let rust_code = codegen
            .try_generate(&main_module.ast)
            .map_err(|e| CliError::failure(format!("Code generation error: {}", e)))?;
        generator
            .generate(&rust_code)
            .map_err(|e| CliError::failure(format!("Error generating project: {}", e)))?;
    }

    Ok(PreparedProject {
        generator,
        out_dir,
        project_root,
    })
}

/// Maximum source file size (100 MB)
///
/// Files larger than this are rejected to prevent out-of-memory conditions
/// during compilation.
const MAX_SOURCE_SIZE: u64 = 100 * 1024 * 1024;

/// Validate the output directory to prevent path traversal attacks.
///
/// This function ensures:
/// - The path doesn't contain `..` components
/// - The path doesn't start with `/` (absolute path outside workspace) unless it starts with a known safe prefix
fn validate_output_dir(out_dir: &str) -> CliResult<()> {
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
/// The stdlib prelude (`stdlib/prelude.incn`) exists but is not currently wired
/// into the compilation pipeline. Prelude traits like `Debug`, `Display`, `Clone`
/// are recognized by codegen heuristics rather than actual trait definitions.
///
/// Future work: integrate prelude ASTs into typechecking so trait bounds are
/// validated and derives work through actual trait implementations.
pub fn collect_modules(entry_path: &str) -> CliResult<Vec<ParsedModule>> {
    let path = Path::new(entry_path);
    let base_dir = path.parent().unwrap_or(Path::new("."));

    let mut modules = Vec::new();
    let mut processed = HashSet::new();
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
                    if path.segments.is_empty() || path.segments.first() == Some(&"std".to_string()) {
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
            source,
            ast,
        });
    }

    modules.reverse();
    Ok(modules)
}

/// Lex and display tokens.
pub fn lex_file(file_path: &str) -> CliResult<ExitCode> {
    let source = read_source(file_path)?;
    let tokens = match lexer::lex(&source) {
        Ok(toks) => toks,
        Err(errs) => {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(file_path, &source, err));
            }
            return Err(CliError::failure(msg.trim_end()));
        }
    };

    for tok in &tokens {
        println!("{:?}", tok);
    }
    Ok(ExitCode::SUCCESS)
}

/// Parse and display AST.
pub fn parse_file(file_path: &str) -> CliResult<ExitCode> {
    let source = read_source(file_path)?;
    let tokens = match lexer::lex(&source) {
        Ok(t) => t,
        Err(errs) => {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(file_path, &source, err));
            }
            return Err(CliError::failure(msg.trim_end()));
        }
    };

    match parser::parse(&tokens) {
        Ok(ast) => {
            println!("{:#?}", ast);
            Ok(ExitCode::SUCCESS)
        }
        Err(errs) => {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(file_path, &source, err));
            }
            Err(CliError::failure(msg.trim_end()))
        }
    }
}

/// Type check a file.
pub fn check_file(file_path: &str) -> CliResult<ExitCode> {
    let modules = collect_modules(file_path)?;

    let Some(main_module) = modules.last() else {
        return Err(CliError::failure("No modules found"));
    };

    let deps: Vec<(&str, &Program)> = modules[..modules.len() - 1]
        .iter()
        .map(|m| (m.name.as_str(), &m.ast))
        .collect();

    let mut checker = typechecker::TypeChecker::new();
    match checker.check_with_imports(&main_module.ast, &deps) {
        Ok(()) => {
            println!("✓ Type check passed!");
            Ok(ExitCode::SUCCESS)
        }
        Err(errs) => {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(file_path, &main_module.source, err));
            }
            Err(CliError::failure(msg.trim_end()))
        }
    }
}

/// Emit generated Rust code.
///
/// If `strict` is true, the output uses stricter clippy attributes to produce
/// warning-clean code suitable for direct use in Rust projects.
pub fn emit_rust(file_path: &str, strict: bool) -> CliResult<ExitCode> {
    let modules = collect_modules(file_path)?;

    let Some(main_module) = modules.last() else {
        return Err(CliError::failure("No modules found"));
    };

    let mut codegen = IrCodegen::new();

    for module in &modules[..modules.len() - 1] {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }

    let rust_code = codegen
        .try_generate(&main_module.ast)
        .map_err(|e| CliError::failure(format!("Code generation error: {}", e)))?;

    // In strict mode, replace permissive allow attributes with stricter ones
    let output = if strict {
        rust_code.replace(
            "#![allow(unused_imports, unused_parens, dead_code, unused_variables, unused_mut, unused_assignments)]",
            "#![deny(unused_imports, unused_variables)]",
        )
    } else {
        rust_code
    };

    println!("{}", output);
    Ok(ExitCode::SUCCESS)
}

/// Build an Incan file to a Rust project.
pub fn build_file(file_path: &str, output_dir: Option<&String>) -> CliResult<ExitCode> {
    let prepared = prepare_project(file_path, output_dir.map(|s| s.as_str()))?;

    println!("Generated Rust project in: {}", prepared.out_dir);
    println!("Building...");

    match prepared.generator.build() {
        Ok(result) => {
            if result.success {
                println!("✓ Build successful!");
                println!("Binary: {}", prepared.generator.binary_path().display());
                Ok(ExitCode::SUCCESS)
            } else {
                Err(CliError::failure(format!("Build failed:\n{}", result.stderr)))
            }
        }
        Err(e) => Err(CliError::failure(format!("Error running cargo: {}", e))),
    }
}

/// Build and run an Incan file.
pub fn run_file(file_path: &str) -> CliResult<ExitCode> {
    let prepared = prepare_project(file_path, None)?;

    match prepared.generator.run_with_cwd(&prepared.project_root) {
        Ok(result) => {
            if !result.stdout.is_empty() {
                print!("{}", result.stdout);
            }
            if !result.stderr.is_empty() && !result.success {
                eprint!("{}", result.stderr);
            }
            // Return the program's exit code
            Ok(ExitCode(result.exit_code.unwrap_or(0)))
        }
        Err(e) => Err(CliError::failure(format!("Error running program: {}", e))),
    }
}

/// Format Incan source files.
pub fn format_files(path: &str, check_mode: bool, diff_mode: bool) -> CliResult<ExitCode> {
    let path = Path::new(path);
    let files = collect_incn_files(path);

    if files.is_empty() {
        return Err(CliError::failure("No .incn files found"));
    }

    let mut needs_formatting = false;
    let mut formatted_count = 0;
    let mut error_count = 0;

    for file_path in &files {
        let source = match fs::read_to_string(file_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading {}: {}", file_path.display(), e);
                error_count += 1;
                continue;
            }
        };

        match format_source(&source) {
            Ok(formatted) => {
                let changed = source != formatted;

                if diff_mode && changed {
                    println!("--- {}", file_path.display());
                    if let Ok(Some(diff)) = format_diff(&source) {
                        print!("{}", diff);
                    }
                    println!();
                }

                if check_mode {
                    if changed {
                        println!("Would reformat: {}", file_path.display());
                        needs_formatting = true;
                    }
                } else if diff_mode {
                    if changed {
                        needs_formatting = true;
                    }
                } else if changed {
                    if let Err(e) = fs::write(file_path, &formatted) {
                        eprintln!("Error writing {}: {}", file_path.display(), e);
                        error_count += 1;
                    } else {
                        println!("Formatted: {}", file_path.display());
                        formatted_count += 1;
                    }
                }
            }
            Err(e) => {
                eprintln!("Error formatting {}: {}", file_path.display(), e);
                error_count += 1;
            }
        }
    }

    if check_mode || diff_mode {
        if needs_formatting {
            let msg = if diff_mode {
                "need formatting"
            } else {
                "would be reformatted"
            };
            return Err(CliError::failure(format!("\n{} file(s) {}", files.len(), msg)));
        } else {
            println!("✓ {} file(s) already formatted", files.len());
        }
    } else {
        println!("\n✓ {} file(s) formatted, {} error(s)", formatted_count, error_count);
    }

    if error_count > 0 {
        return Err(CliError::new("", ExitCode::FAILURE));
    }

    Ok(ExitCode::SUCCESS)
}

fn collect_incn_files(path: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    if path.is_file() {
        if path.extension().is_some_and(|ext| ext == "incn") {
            files.push(path.to_path_buf());
        }
    } else if path.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                if entry_path.is_dir() {
                    let name = entry_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') && name != "target" && name != "node_modules" {
                        files.extend(collect_incn_files(&entry_path));
                    }
                } else if entry_path.extension().is_some_and(|ext| ext == "incn") {
                    files.push(entry_path);
                }
            }
        }
    }

    files
}

/// Collect Rust crate names from imports
pub fn collect_rust_crates(ast: &crate::frontend::ast::Program) -> Vec<String> {
    use crate::frontend::ast::ImportKind;

    let mut crates = Vec::new();

    for decl in &ast.declarations {
        if let crate::frontend::ast::Declaration::Import(import) = &decl.node {
            match &import.kind {
                ImportKind::RustCrate { crate_name, .. } => {
                    if crate_name != stdlib::STDLIB_ROOT && !crates.contains(crate_name) {
                        crates.push(crate_name.clone());
                    }
                }
                ImportKind::RustFrom { crate_name, .. } => {
                    if crate_name != stdlib::STDLIB_ROOT && !crates.contains(crate_name) {
                        crates.push(crate_name.clone());
                    }
                }
                _ => {}
            }
        }
    }

    crates
}
