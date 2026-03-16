use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::commands::common::resolve_stdlib_module_source_path;
use crate::cli::prelude::ParsedModule;
use crate::frontend::ast::{Declaration, ImportKind, Program};
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::vocab_desugar_pass;
use crate::frontend::{diagnostics, lexer, parser};
use incan_core::lang::stdlib;

/// Resolve and queue an emitted Incan stdlib source module for recursive test dependency collection.
///
/// This helper:
/// - memoizes `std.*` source-path resolution so repeated imports do not hit the filesystem repeatedly
/// - rewrites the logical module path to the emitted `__incan_std::*` namespace used by generated Rust
/// - avoids re-queueing modules that have already been processed
fn queue_incan_stdlib_source_module(
    module_path: &[String],
    incan_source_stdlib_module_paths: &mut HashMap<String, PathBuf>,
    processed: &HashSet<PathBuf>,
    to_process: &mut Vec<(PathBuf, String, Vec<String>)>,
) -> Result<(), String> {
    let stdlib_key = module_path.join(".");
    let source_path = if let Some(cached_path) = incan_source_stdlib_module_paths.get(&stdlib_key) {
        cached_path.clone()
    } else {
        let resolved = resolve_stdlib_module_source_path(module_path).map_err(|err| err.message)?;
        incan_source_stdlib_module_paths.insert(stdlib_key, resolved.clone());
        resolved
    };
    let mut module_segments = vec![stdlib::INCAN_STD_NAMESPACE.to_string()];
    module_segments.extend(module_path.iter().skip(1).cloned());
    let module_name = module_segments.join("_");
    if !processed.contains(&source_path) {
        to_process.push((source_path, module_name, module_segments));
    }
    Ok(())
}

/// Collect source modules referenced by a test file's imports.
///
/// Walks the test AST for `from <module> import ...` statements that reference user modules and materialized stdlib
/// sources. Each is resolved against the project's source root so that `from greet import greet` in a test finds
/// `src/greet.incn` — the same file that `src/main.incn` uses.
///
/// Collected modules include transitive dependencies (e.g. if `greet.incn` imports `utils.incn`, it is collected too).
pub(super) fn collect_source_modules_for_test(
    test_ast: &Program,
    source_root: &Path,
    library_imported_vocab: Option<&parser::ImportedLibraryVocab>,
    library_manifest_index: Option<&LibraryManifestIndex>,
) -> Result<Vec<ParsedModule>, String> {
    let mut modules = Vec::new();
    let mut processed = HashSet::new();
    let mut to_process: Vec<(PathBuf, String, Vec<String>)> = Vec::new();
    let mut incan_source_stdlib_module_paths: HashMap<String, PathBuf> = HashMap::new();

    // ---- Walk test AST to find user module imports ----
    for decl in &test_ast.declarations {
        let Declaration::Import(import) = &decl.node else {
            continue;
        };

        let import_path = match &import.kind {
            ImportKind::From { module, .. } if !module.segments.is_empty() => Some(module),
            ImportKind::Module(path) if !path.segments.is_empty() => Some(path),
            _ => continue,
        };

        let Some(path) = import_path else { continue };

        if path.parent_levels == 0
            && !path.is_absolute
            && path
                .segments
                .first()
                .is_some_and(|segment| segment == stdlib::STDLIB_ROOT)
        {
            if stdlib::stdlib_stub_path(&path.segments).is_some() {
                queue_incan_stdlib_source_module(
                    &path.segments,
                    &mut incan_source_stdlib_module_paths,
                    &processed,
                    &mut to_process,
                )?;
            }
            continue;
        }

        // Skip other stdlib and rust crate imports.
        if path
            .segments
            .first()
            .is_some_and(|s| s == stdlib::STDLIB_ROOT || s == "rust")
        {
            continue;
        }
        // Skip relative imports (e.g. `from ..src.greet`) — those are the old pattern.
        if path.parent_levels > 0 || path.is_absolute {
            continue;
        }

        let module_segments = match &import.kind {
            ImportKind::From { module, .. } => module.segments.clone(),
            ImportKind::Module(p) => {
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

        // Resolve against the source root.
        let mut file_path = source_root.to_path_buf();
        for seg in &module_segments {
            file_path = file_path.join(seg);
        }
        file_path.set_extension("incn");

        if !file_path.exists() {
            // Try .incan extension as fallback.
            file_path.set_extension("incan");
            if !file_path.exists() {
                continue; // Not a source module — might be a built-in or typo.
            }
        }

        let module_name = module_segments.join("_");
        if !processed.contains(&file_path) {
            to_process.push((file_path, module_name, module_segments));
        }
    }

    // ---- Recursively collect modules and their transitive dependencies ----
    while let Some((file_path, module_name, path_segments)) = to_process.pop() {
        if processed.contains(&file_path) {
            continue;
        }
        processed.insert(file_path.clone());

        let source = fs::read_to_string(&file_path)
            .map_err(|e| format!("Failed to read source module '{}': {}", file_path.display(), e))?;

        let tokens = lexer::lex(&source).map_err(|errs| {
            let mut msg = String::new();
            let fp = file_path.to_string_lossy();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(&fp, &source, err));
            }
            msg
        })?;

        let fp = file_path.to_string_lossy();
        let mut ast =
            parser::parse_with_context(&tokens, Some(fp.as_ref()), library_imported_vocab).map_err(|errs| {
                let mut msg = String::new();
                for err in &errs {
                    msg.push_str(&diagnostics::format_error(&fp, &source, err));
                }
                msg
            })?;
        if let Some(index) = library_manifest_index {
            vocab_desugar_pass::desugar_program_vocab_blocks(&mut ast, Some(fp.as_ref()), index).map_err(|errs| {
                let mut msg = String::new();
                for err in &errs {
                    msg.push_str(&diagnostics::format_error(&fp, &source, err));
                }
                msg
            })?;
        }
        // Surface any non-fatal parser warnings immediately.
        for warn in &ast.warnings {
            let fp = file_path.to_string_lossy();
            eprint!("{}", diagnostics::format_error(&fp, &source, warn));
        }

        // Walk this module's imports for transitive dependencies.
        for decl in &ast.declarations {
            let Declaration::Import(import) = &decl.node else {
                continue;
            };
            let dep_path = match &import.kind {
                ImportKind::From { module, .. } if !module.segments.is_empty() => Some(module),
                ImportKind::Module(p) if !p.segments.is_empty() => Some(p),
                _ => continue,
            };
            let Some(dep) = dep_path else { continue };
            if dep.parent_levels == 0
                && !dep.is_absolute
                && dep
                    .segments
                    .first()
                    .is_some_and(|segment| segment == stdlib::STDLIB_ROOT)
            {
                if stdlib::stdlib_stub_path(&dep.segments).is_some() {
                    queue_incan_stdlib_source_module(
                        &dep.segments,
                        &mut incan_source_stdlib_module_paths,
                        &processed,
                        &mut to_process,
                    )?;
                }
                continue;
            }
            if dep
                .segments
                .first()
                .is_some_and(|s| s == stdlib::STDLIB_ROOT || s == "rust")
            {
                continue;
            }
            if dep.parent_levels > 0 || dep.is_absolute {
                continue;
            }

            let dep_segments = match &import.kind {
                ImportKind::From { module, .. } => module.segments.clone(),
                ImportKind::Module(p) => {
                    if p.segments.len() > 1 {
                        p.segments[..p.segments.len() - 1].to_vec()
                    } else {
                        p.segments.clone()
                    }
                }
                _ => continue,
            };

            if dep_segments.is_empty() {
                continue;
            }

            let mut dep_file = source_root.to_path_buf();
            for seg in &dep_segments {
                dep_file = dep_file.join(seg);
            }
            dep_file.set_extension("incn");
            if !dep_file.exists() {
                dep_file.set_extension("incan");
                if !dep_file.exists() {
                    continue;
                }
            }

            let dep_name = dep_segments.join("_");
            if !processed.contains(&dep_file) {
                to_process.push((dep_file, dep_name, dep_segments));
            }
        }

        modules.push(ParsedModule {
            name: module_name,
            path_segments,
            file_path,
            source,
            ast,
        });
    }

    Ok(modules)
}
