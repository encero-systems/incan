use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::commands::common::{
    resolve_stdlib_module_source_path, topologically_sort_modules, uses_iterator_adapter_surface,
    uses_result_combinator_surface,
};
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
) -> Result<Option<PathBuf>, String> {
    // The compiled artifact owns this module's generated Rust implementation. Its checked manifest is loaded by the
    // test preparation path, so materializing source here would create a second `__incan_std` tree in the harness.
    if stdlib::is_compiled_builtin_stdlib_module(module_path) {
        return Ok(None);
    }
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
        to_process.push((source_path.clone(), module_name, module_segments));
    }
    Ok(Some(source_path))
}

/// Queue one canonical source-import resolution for test dependency collection.
fn queue_resolved_source_import(
    resolution: SourceModuleImportResolution,
    incan_source_stdlib_module_paths: &mut HashMap<String, PathBuf>,
    processed: &HashSet<PathBuf>,
    to_process: &mut Vec<(PathBuf, String, Vec<String>)>,
) -> Result<Option<PathBuf>, String> {
    match resolution {
        SourceModuleImportResolution::Stdlib { module_path } => {
            if stdlib::stdlib_stub_path(&module_path).is_some() {
                return queue_incan_stdlib_source_module(
                    &module_path,
                    incan_source_stdlib_module_paths,
                    processed,
                    to_process,
                );
            }
        }
        SourceModuleImportResolution::Local(module_ref) => {
            if !processed.contains(&module_ref.file_path) {
                to_process.push((
                    module_ref.file_path.clone(),
                    module_ref.module_name,
                    module_ref.path_segments,
                ));
            }
            return Ok(Some(module_ref.file_path));
        }
        SourceModuleImportResolution::External => {}
    }
    Ok(None)
}

/// Queue implicit source stdlib helper modules that generated Rust may reference without a source import.
fn queue_implicit_stdlib_helpers(
    program: &Program,
    incan_source_stdlib_module_paths: &mut HashMap<String, PathBuf>,
    processed: &HashSet<PathBuf>,
    to_process: &mut Vec<(PathBuf, String, Vec<String>)>,
) -> Result<Vec<PathBuf>, String> {
    let mut queued = Vec::new();
    if uses_iterator_adapter_surface(program)
        && let Some(path) = queue_incan_stdlib_source_module(
            &[
                stdlib::STDLIB_ROOT.to_string(),
                "derives".to_string(),
                "collection".to_string(),
            ],
            incan_source_stdlib_module_paths,
            processed,
            to_process,
        )?
    {
        queued.push(path);
    }
    if uses_result_combinator_surface(program)
        && let Some(path) = queue_incan_stdlib_source_module(
            &[stdlib::STDLIB_ROOT.to_string(), "result".to_string()],
            incan_source_stdlib_module_paths,
            processed,
            to_process,
        )?
    {
        queued.push(path);
    }
    Ok(queued)
}

/// Return the stable key used for dependency graph edges.
fn dependency_edge_key(path: &Path) -> String {
    path.to_string_lossy().to_string()
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
    let mut dependency_edges: HashMap<String, HashSet<String>> = HashMap::new();

    queue_implicit_stdlib_helpers(
        test_ast,
        &mut incan_source_stdlib_module_paths,
        &processed,
        &mut to_process,
    )?;

    // ---- Walk test AST to find user module imports ----
    for resolved in resolve_program_source_imports(test_ast, source_root, Some(source_root)) {
        let _ = queue_resolved_source_import(
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
        let file_key = dependency_edge_key(&file_path);
        dependency_edges.entry(file_key.clone()).or_default();

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

        for dependency_path in
            queue_implicit_stdlib_helpers(&ast, &mut incan_source_stdlib_module_paths, &processed, &mut to_process)?
        {
            dependency_edges
                .entry(file_key.clone())
                .or_default()
                .insert(dependency_edge_key(&dependency_path));
        }

        // Walk this module's imports for transitive dependencies.
        let current_base = file_path.parent().unwrap_or(source_root);
        for resolved in resolve_program_source_imports(&ast, current_base, Some(source_root)) {
            if let Some(dependency_path) = queue_resolved_source_import(
                resolved.resolution,
                &mut incan_source_stdlib_module_paths,
                &processed,
                &mut to_process,
            )? {
                dependency_edges
                    .entry(file_key.clone())
                    .or_default()
                    .insert(dependency_edge_key(&dependency_path));
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

    topologically_sort_modules(modules, &dependency_edges).map_err(|err| err.message)
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

    #[test]
    fn test_runner_orders_source_dependencies_before_dependents() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let src_dir = tmp.path().join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(src_dir.join("helper.incn"), "pub def target() -> int:\n    return 1\n")?;
        std::fs::write(
            src_dir.join("functions.incn"),
            "from helper import target as target_builder\n\npub public_target = alias target_builder\n",
        )?;

        let test_source = "from functions import public_target\n";
        let tokens = lexer::lex(test_source).map_err(|errs| errs[0].message.clone())?;
        let ast = parser::parse_with_context(&tokens, Some("tests/test_alias.incn"), None)
            .map_err(|errs| errs[0].message.clone())?;

        let modules = collect_source_modules_for_test(&ast, &src_dir, None, None, None)?;
        let helper_idx = modules
            .iter()
            .position(|module| module.file_path.ends_with("helper.incn"))
            .ok_or("expected helper.incn to be collected")?;
        let functions_idx = modules
            .iter()
            .position(|module| module.file_path.ends_with("functions.incn"))
            .ok_or("expected functions.incn to be collected")?;

        assert!(
            helper_idx < functions_idx,
            "test runner should order dependency modules before dependent modules"
        );
        Ok(())
    }

    #[test]
    fn test_runner_uses_compiled_result_helper_without_materializing_source() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let src_dir = tmp.path().join("src");
        std::fs::create_dir_all(&src_dir)?;

        let test_source = r#"
def produce_error() -> Result[int, str]:
    return Err("bad")


def convert_error(err: str) -> int:
    return len(err)


def test_map_err_result_helper_is_packaged() -> None:
    match produce_error().map_err(convert_error):
        Ok(_) => assert false
        Err(code) => assert code == 3
"#;
        let tokens = lexer::lex(test_source).map_err(|errs| errs[0].message.clone())?;
        let ast = parser::parse_with_context(&tokens, Some("tests/test_result_map_err.incn"), None)
            .map_err(|errs| errs[0].message.clone())?;

        let modules = collect_source_modules_for_test(&ast, &src_dir, None, None, None)?;

        assert!(
            !modules.iter().any(|module| module.path_segments
                == vec![
                    incan_core::lang::stdlib::INCAN_STD_NAMESPACE.to_string(),
                    "result".to_string()
                ]),
            "compiled std.result helper source must not be collected, got {:?}",
            modules
                .iter()
                .map(|module| module.path_segments.clone())
                .collect::<Vec<_>>()
        );
        Ok(())
    }
}
