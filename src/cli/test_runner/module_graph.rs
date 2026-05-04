use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::commands::common::resolve_stdlib_module_source_path;
use crate::cli::prelude::ParsedModule;
use crate::frontend::ast::Program;
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::module::{SourceModuleImportResolution, resolve_program_source_imports};
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

/// Queue one canonical source-import resolution for test dependency collection.
fn queue_resolved_source_import(
    resolution: SourceModuleImportResolution,
    incan_source_stdlib_module_paths: &mut HashMap<String, PathBuf>,
    processed: &HashSet<PathBuf>,
    to_process: &mut Vec<(PathBuf, String, Vec<String>)>,
) -> Result<(), String> {
    match resolution {
        SourceModuleImportResolution::Stdlib { module_path } => {
            if stdlib::stdlib_stub_path(&module_path).is_some() {
                queue_incan_stdlib_source_module(
                    &module_path,
                    incan_source_stdlib_module_paths,
                    processed,
                    to_process,
                )?;
            }
        }
        SourceModuleImportResolution::Local(module_ref) => {
            if !processed.contains(&module_ref.file_path) {
                to_process.push((module_ref.file_path, module_ref.module_name, module_ref.path_segments));
            }
        }
        SourceModuleImportResolution::External => {}
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
pub(crate) fn collect_source_modules_for_test(
    test_ast: &Program,
    source_root: &Path,
    library_imported_vocab: Option<&parser::ImportedLibraryVocab>,
    library_imported_dsl_surfaces: Option<&parser::ImportedLibraryDslSurfaces>,
    library_manifest_index: Option<&LibraryManifestIndex>,
) -> Result<Vec<ParsedModule>, String> {
    let mut modules = Vec::new();
    let mut processed = HashSet::new();
    let mut to_process: Vec<(PathBuf, String, Vec<String>)> = Vec::new();
    let mut incan_source_stdlib_module_paths: HashMap<String, PathBuf> = HashMap::new();

    // ---- Walk test AST to find user module imports ----
    for resolved in resolve_program_source_imports(test_ast, source_root, Some(source_root)) {
        queue_resolved_source_import(
            resolved.resolution,
            &mut incan_source_stdlib_module_paths,
            &processed,
            &mut to_process,
        )?;
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
        let mut ast = parser::parse_with_context_and_surfaces(
            &tokens,
            Some(fp.as_ref()),
            library_imported_vocab,
            library_imported_dsl_surfaces,
        )
        .map_err(|errs| {
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
        let current_base = file_path.parent().unwrap_or(source_root);
        for resolved in resolve_program_source_imports(&ast, current_base, Some(source_root)) {
            queue_resolved_source_import(
                resolved.resolution,
                &mut incan_source_stdlib_module_paths,
                &processed,
                &mut to_process,
            )?;
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

#[cfg(test)]
mod tests {
    use super::collect_source_modules_for_test;
    use crate::frontend::{lexer, parser};
    use std::path::Path;

    #[test]
    fn test_runner_collects_nested_package_modules() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let src_dir = project_root.join("src");
        let tests_dir = project_root.join("tests");
        std::fs::create_dir_all(src_dir.join("dataset"))?;
        std::fs::create_dir_all(&tests_dir)?;

        std::fs::write(
            src_dir.join("dataset").join("mod.incn"),
            "pub trait DataSet[T]:\n    pass\n",
        )?;
        std::fs::write(
            src_dir.join("dataset").join("ops.incn"),
            "from dataset.mod import DataSet\npub def filter_ds[T](ds: DataSet[T]) -> DataSet[T]:\n    return ds\n",
        )?;
        let test_source = "from dataset.mod import DataSet\nfrom dataset.ops import filter_ds\n";
        let tokens = lexer::lex(test_source).map_err(|errs| errs[0].message.clone())?;
        let ast = parser::parse_with_context(&tokens, Some("tests/test_dataset.incn"), None)
            .map_err(|errs| errs[0].message.clone())?;

        let modules = collect_source_modules_for_test(&ast, &src_dir, None, None, None)?;

        let dataset_mod = modules
            .iter()
            .find(|module| module.file_path.ends_with(Path::new("dataset").join("mod.incn")))
            .ok_or("expected dataset/mod.incn to be collected")?;
        assert_eq!(dataset_mod.path_segments, vec!["dataset".to_string()]);

        let dataset_ops = modules
            .iter()
            .find(|module| module.file_path.ends_with(Path::new("dataset").join("ops.incn")))
            .ok_or("expected dataset/ops.incn to be collected")?;
        assert_eq!(
            dataset_ops.path_segments,
            vec!["dataset".to_string(), "ops".to_string()]
        );

        Ok(())
    }
}
