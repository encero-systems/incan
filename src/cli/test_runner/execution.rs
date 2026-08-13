use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::backend::{IrCodegen, ProjectGenerator};
use crate::cli::commands;
use crate::cli::commands::common::{self, CargoPolicy, ProjectRequirements};
#[cfg(feature = "rust_inspect")]
use crate::cli::commands::lock::{
    OvenRustInspectSourceAuthorityRequest, RustInspectWorkspaceRequest, prepare_rust_inspect_workspace,
};
use crate::cli::prelude::ParsedModule;
use crate::compiled_sdk::CompiledSdkModules;
use crate::dependency_resolver::ResolvedDependencies;
use crate::dependency_resolver::resolve_reachable_dependencies;
use crate::frontend::ast::{
    AssertKind, AssertStmt, CallArg, Declaration, DictEntry, Expr, ImportItem, ImportKind, ListEntry, ParamKind,
    Program, Span, Spanned, Statement, Type,
};
use crate::frontend::decorator_resolution;
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::module::logical_module_segments_from_file;
use crate::frontend::testing_markers::{TestingMarkerKind, TestingMarkerSemantics, resolve_testing_marker_kind};
use crate::frontend::vocab_desugar_pass;
use crate::frontend::{lexer, parser};
use crate::lockfile::CargoFeatureSelection;
use crate::manifest::DependencySpec;
use crate::oven::loaf::{OVEN_LOAF_MISS_GUIDANCE, runtime_build_unit_inputs};
use crate::oven::native_test::{OvenNativeTestRequest, run_native_test_batch};
use crate::oven::rustc::{
    OvenTrustedDirectRustcTargetRequest, attach_caller_owned_rustc_libraries, bake_trusted_direct_rustc_test,
    materialize_declared_rust_libraries_with_selected_path_authority, resolve_active_rustc, rustc_host_target,
    rustc_identity, trusted_artifact_plan_for_source_evidence,
};
use crate::oven::{
    OvenGeneratedProjectRequest, default_receipt_path, digest_dependency_specs, receipt_generated_project,
    write_receipt,
};
use crate::provider::{FeatureSelection, ProviderPlan};
use sha2::{Digest, Sha256};

use super::module_graph::collect_source_modules_for_test;
use super::types::{FixtureScope, TestInfo, TestResult};
use crate::cli::commands::lock::validate_oven_lock_policy;

/// Generated `#[cfg(test)]` module that wraps Incan test functions as Rust `#[test]` cases.
const INCAN_FILE_TEST_MOD: &str = "__incan_file_tests";
const INCAN_SESSION_FIXTURE_MOD: &str = "__incan_session_fixtures";

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TestExecutionOptions {
    pub no_capture: bool,
    pub timeout: Option<Duration>,
    pub verbose: bool,
    pub emit_progress: bool,
}

/// Receipt-selected direct-Rustc closure for a nested normal test.
///
/// A compiler-suite child receives a parent-leased immutable compiler-data root. It uses that Loaf directly instead
/// of publishing a duplicate closure into its output-owned store; ordinary tests keep the bounded-store path.
type OvenTestPlanSelection = crate::cli::commands::build::OvenDirectRustcPlanSelection;

/// Validate strict Incan lock policy once before Oven schedules any generated native test harnesses.
///
/// `--locked` and `--frozen` remain compiler-owned lock-consistency promises after normal test execution leaves
/// Cargo. Validation uses the Oven read-only resolver: a missing or stale lock fails before scheduling, and a
/// normal test command never publishes SDK/provider or dependency artifacts.
pub(super) fn validate_oven_test_lock_policy(
    representative_test: &Path,
    cargo_policy: &CargoPolicy,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> crate::cli::CliResult<()> {
    if !cargo_policy.locked && !cargo_policy.frozen {
        return Ok(());
    }

    let session =
        common::CompilationSession::discover_for_oven(representative_test, package_features, sdk_profile_override)?;
    let manifest = session.manifest.clone();
    let inferred_project_root = common::resolve_project_root(representative_test);
    let project_root = manifest
        .as_ref()
        .map(|manifest| manifest.project_root().to_path_buf())
        .unwrap_or(inferred_project_root);
    let cargo_features = CargoFeatureSelection::default().normalized();
    validate_oven_lock_policy(
        &project_root,
        manifest.as_ref(),
        representative_test,
        &cargo_features,
        cargo_policy,
        package_features,
        sdk_profile_override,
    )
}

/// Collect inline imports required by dependencies of a test source file.
fn collect_test_dependency_inline_imports(
    test_module: &ParsedModule,
    source_modules: &[ParsedModule],
) -> Vec<crate::dependency_resolver::InlineRustImport> {
    let mut inline_imports = common::collect_rust_dependency_uses(test_module, true);
    for module in source_modules {
        inline_imports.extend(common::collect_rust_dependency_uses(module, false));
    }
    inline_imports
}

/// Return a runner-only AST where RFC 018 inline test-module declarations are emitted as ordinary module declarations.
///
/// Production build/run lowering intentionally strips `Declaration::TestModule`. The test runner needs the opposite:
/// the production declarations plus the inline test declarations in one generated test crate so the existing per-file
/// Rust harness can call inline `test_*` functions directly.
fn ast_with_inline_test_declarations(ast: &Program) -> Program {
    let mut declarations = Vec::with_capacity(ast.declarations.len());
    for decl in &ast.declarations {
        match &decl.node {
            Declaration::TestModule(test_module) => declarations.extend(test_module.body.iter().cloned()),
            _ => declarations.push(decl.clone()),
        }
    }

    Program {
        declarations,
        source_path: ast.source_path.clone(),
        rust_module_path: ast.rust_module_path.clone(),
        warnings: ast.warnings.clone(),
    }
}

/// Return whether a top-level function is a `std.testing.fixture` declaration.
fn has_fixture_decorator(
    decorators: &[crate::frontend::ast::Spanned<crate::frontend::ast::Decorator>],
    aliases: &HashMap<String, Vec<String>>,
    semantics: &TestingMarkerSemantics,
) -> bool {
    decorators.iter().any(|decorator| {
        resolve_testing_marker_kind(&decorator.node, aliases, semantics) == Some(TestingMarkerKind::Fixture)
    })
}

/// Remove shadowed fixture functions so execution uses the same "nearest fixture wins" rule as collection.
fn prune_shadowed_fixture_declarations(ast: &mut Program, semantics: &TestingMarkerSemantics) {
    let aliases = decorator_resolution::collect_import_aliases(ast);
    let mut last_fixture_decl = HashMap::new();
    for (index, decl) in ast.declarations.iter().enumerate() {
        if let Declaration::Function(func) = &decl.node
            && has_fixture_decorator(&func.decorators, &aliases, semantics)
        {
            last_fixture_decl.insert(func.name.clone(), index);
        }
    }

    ast.declarations = ast
        .declarations
        .iter()
        .enumerate()
        .filter(|(index, decl)| {
            if let Declaration::Function(func) = &decl.node
                && has_fixture_decorator(&func.decorators, &aliases, semantics)
            {
                return last_fixture_decl.get(&func.name) == Some(index);
            }
            true
        })
        .map(|(_, decl)| decl.clone())
        .collect();
}

/// Build a stable de-duplication key for one imported item under an import declaration prefix.
fn import_item_key(prefix: &str, item: &ImportItem) -> String {
    format!("{prefix}:{}:{:?}", item.name, item.alias)
}

/// Drop repeated import bindings introduced by concatenating inherited conftests.
fn dedupe_import_declarations(ast: &mut Program) {
    let mut seen_imports = Vec::new();
    let mut declarations = Vec::with_capacity(ast.declarations.len());

    for mut decl in ast.declarations.drain(..) {
        let keep = match &mut decl.node {
            Declaration::Import(import) => match &mut import.kind {
                ImportKind::From { module, items } => {
                    let prefix = format!("from:{:?}:{:?}", import.visibility, module);
                    items.retain(|item| {
                        let key = import_item_key(&prefix, item);
                        if seen_imports.contains(&key) {
                            false
                        } else {
                            seen_imports.push(key);
                            true
                        }
                    });
                    !items.is_empty()
                }
                ImportKind::PubFrom { library, path, items } => {
                    let prefix = format!("pub-from:{:?}:{library}:{path:?}", import.visibility);
                    items.retain(|item| {
                        let key = import_item_key(&prefix, item);
                        if seen_imports.contains(&key) {
                            false
                        } else {
                            seen_imports.push(key);
                            true
                        }
                    });
                    !items.is_empty()
                }
                ImportKind::RustFrom {
                    crate_name,
                    path,
                    version,
                    features,
                    items,
                } => {
                    let prefix = format!(
                        "rust-from:{:?}:{crate_name}:{path:?}:{version:?}:{features:?}",
                        import.visibility
                    );
                    items.retain(|item| {
                        let key = import_item_key(&prefix, item);
                        if seen_imports.contains(&key) {
                            false
                        } else {
                            seen_imports.push(key);
                            true
                        }
                    });
                    !items.is_empty()
                }
                _ => {
                    let key = format!("import:{import:?}");
                    if seen_imports.contains(&key) {
                        false
                    } else {
                        seen_imports.push(key);
                        true
                    }
                }
            },
            _ => true,
        };

        if keep {
            declarations.push(decl);
        }
    }

    ast.declarations = declarations;
}

/// Shift token spans after concatenating test source files.
fn rebase_token_spans(tokens: &mut [lexer::Token], source_offset: usize) {
    if source_offset == 0 {
        return;
    }

    for token in tokens {
        token.span.start = token.span.start.saturating_add(source_offset);
        token.span.end = token.span.end.saturating_add(source_offset);
        if let lexer::TokenKind::FString(parts) = &mut token.kind {
            for part in parts {
                if let lexer::FStringPart::Expr { offset, .. } = part {
                    *offset = offset.saturating_add(source_offset);
                }
            }
        }
    }
}

/// Parse each source file in a generated test batch independently, then merge declarations for the shared harness.
///
/// The parser's `module tests:` cardinality rule is intentionally per source file. A worker batch may contain several
/// files, so the runner must not concatenate source text and ask the parser to treat that batch as one file.
fn parse_test_batch_sources(
    batch_sources: &[(PathBuf, String)],
    library_imported_vocab: Option<&parser::ImportedLibraryVocab>,
    library_imported_dsl_surfaces: Option<&parser::ImportedLibraryDslSurfaces>,
    compilation_session: &common::CompilationSession,
) -> Result<Program, String> {
    let mut declarations = Vec::new();
    let mut warnings = Vec::new();
    let mut rust_module_path = None;
    let mut source_offset = 0usize;
    let source_path = batch_sources
        .first()
        .map(|(path, _)| path.to_string_lossy().to_string());

    for (path, source) in batch_sources {
        let mut tokens = lexer::lex(source).map_err(|e| format!("Lexer error in {}: {:?}", path.display(), e))?;
        rebase_token_spans(&mut tokens, source_offset);
        let parsed = parser::parse_with_context_and_surfaces(
            &tokens,
            Some(path.to_string_lossy().as_ref()),
            library_imported_vocab,
            library_imported_dsl_surfaces,
        )
        .map_err(|e| format!("Parser error in {}: {:?}", path.display(), e))?;
        let parsed = compilation_session
            .project_parsed_program(parsed)
            .map_err(|e| format!("Feature projection error in {}: {:?}", path.display(), e))?;
        if let Some(module_path) = parsed.rust_module_path {
            if rust_module_path.is_some() {
                return Err(format!(
                    "Parser error in {}: duplicate rust.module() directives in test batch",
                    path.display()
                ));
            }
            rust_module_path = Some(module_path);
        }
        warnings.extend(parsed.warnings);
        declarations.extend(parsed.declarations);
        source_offset = source_offset.saturating_add(source.len()).saturating_add(1);
    }

    Ok(Program {
        declarations,
        source_path,
        rust_module_path,
        warnings,
    })
}

struct IsolatedSourceModuleBatch {
    ast: Program,
    source_modules: Vec<ParsedModule>,
    harnesses: Vec<PreparedModuleHarness>,
}

/// Create an empty synthetic program for a test batch.
fn empty_test_batch_root(first_path: &Path) -> Program {
    Program {
        declarations: Vec::new(),
        source_path: Some(first_path.to_string_lossy().to_string()),
        rust_module_path: None,
        warnings: Vec::new(),
    }
}

/// Prepare the runner AST and fixture metadata for a test module.
fn prepare_runner_program(
    ast: &Program,
    testing_marker_semantics: Option<&TestingMarkerSemantics>,
) -> Result<(Program, HashMap<String, FixtureExecutionInfo>), String> {
    let mut runner_ast = ast_with_inline_test_declarations(ast);
    normalize_runner_assert_statements(&mut runner_ast);
    let aliases = decorator_resolution::collect_import_aliases(&runner_ast);
    let requires_testing_semantics = runner_ast.declarations.iter().any(|declaration| {
        let Declaration::Function(function) = &declaration.node else {
            return false;
        };
        function.decorators.iter().any(|decorator| {
            let path = decorator_resolution::resolve_decorator_path(&decorator.node, &aliases);
            path.as_slice() == ["std", "testing", "fixture"]
        })
    });
    let default_semantics = TestingMarkerSemantics::default();
    let semantics = match testing_marker_semantics {
        Some(semantics) => semantics,
        None if !requires_testing_semantics => &default_semantics,
        None => return Err("std.testing fixture execution requires the compiled std.testing provider".to_string()),
    };
    prune_shadowed_fixture_declarations(&mut runner_ast, semantics);
    dedupe_import_declarations(&mut runner_ast);
    let mut fixtures = collect_fixture_execution_info(&runner_ast, &HashMap::new(), semantics);
    let fixture_teardowns = split_yield_fixture_declarations(&mut runner_ast, semantics)?;
    apply_fixture_teardowns(&mut fixtures, &fixture_teardowns);
    Ok((runner_ast, fixtures))
}

/// Parse and desugar all source files in a test batch.
fn parse_and_desugar_test_sources(
    batch_sources: &[(PathBuf, String)],
    library_manifest_index: &LibraryManifestIndex,
    library_imported_vocab: &parser::ImportedLibraryVocab,
    library_imported_dsl_surfaces: &parser::ImportedLibraryDslSurfaces,
    compilation_session: &common::CompilationSession,
) -> Result<Program, String> {
    let mut ast = parse_test_batch_sources(
        batch_sources,
        Some(library_imported_vocab),
        Some(library_imported_dsl_surfaces),
        compilation_session,
    )?;
    let path_display = batch_sources
        .last()
        .or_else(|| batch_sources.first())
        .map(|(path, _)| path.to_string_lossy());
    if let Err(errors) =
        vocab_desugar_pass::desugar_program_vocab_blocks(&mut ast, path_display.as_deref(), library_manifest_index)
    {
        return Err(format!("Vocab desugar error: {:?}", errors));
    }
    Ok(ast)
}

/// Build a stable synthetic module name from module path segments.
fn module_name_for_segments(segments: &[String]) -> String {
    let mut hasher = Sha256::new();
    for segment in segments {
        hasher.update(segment.as_bytes());
        hasher.update([0]);
    }
    let digest = hex::encode(hasher.finalize());
    let stem = if segments.is_empty() {
        "module".to_string()
    } else {
        segments.join("_")
    };
    format!("{stem}_{}", &digest[..8])
}

/// Derive a stable generated module path for one test source file.
///
/// Normal package files use their project-relative path, such as `tests.foo`. If a caller supplies an unusual file
/// path outside the known roots, keep the multi-file batch isolated by assigning a synthetic path instead of falling
/// back to concatenating independent test files into one frontend scope.
fn test_module_segments_for_file(project_root: &Path, source_root: &Path, path: &Path) -> Vec<String> {
    let absolute_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };
    if let Some(module_path) = logical_module_segments_from_file(source_root, &absolute_path)
        .or_else(|| logical_module_segments_from_file(project_root, &absolute_path))
    {
        return module_path;
    }

    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("test");
    let mut hasher = Sha256::new();
    hasher.update(canonical_path_for_cache_key(path).to_string_lossy().as_bytes());
    let digest = hex::encode(hasher.finalize());
    vec!["tests".to_string(), format!("{stem}_{}", &digest[..8])]
}

/// Read conftest source files for a test batch.
fn read_conftest_sources(paths: &[PathBuf]) -> Result<Vec<(PathBuf, String)>, String> {
    let mut sources = Vec::new();
    for path in paths {
        let source =
            fs::read_to_string(path).map_err(|err| format!("Failed to read conftest {}: {}", path.display(), err))?;
        sources.push((path.clone(), source));
    }
    Ok(sources)
}

/// Prepare a multi-file test batch that keeps each test file in its own generated Rust module.
///
/// Concatenating independent test files into one frontend program leaks file-local imports and aliases across the
/// whole batch. Module-isolated batching preserves source-file scope while still compiling one shared Cargo harness.
#[allow(clippy::too_many_arguments)] // Batch preparation consumes already-normalized session inputs from the runner.
fn prepare_isolated_source_module_batch(
    sources_by_file: &[(PathBuf, String)],
    conftest_files_by_file: &HashMap<PathBuf, Vec<PathBuf>>,
    project_root: &Path,
    source_root: &Path,
    library_manifest_index: &LibraryManifestIndex,
    library_imported_vocab: &parser::ImportedLibraryVocab,
    library_imported_dsl_surfaces: &parser::ImportedLibraryDslSurfaces,
    compilation_session: &common::CompilationSession,
    testing_marker_semantics: Option<&TestingMarkerSemantics>,
) -> Result<Option<IsolatedSourceModuleBatch>, String> {
    if sources_by_file.len() <= 1 {
        return Ok(None);
    }

    let mut source_modules = Vec::new();
    let mut harnesses = Vec::new();
    let mut batch_files = HashSet::new();
    let mut seen_module_paths = HashSet::new();
    let mut parsed_sources = Vec::new();

    for (path, source) in sources_by_file {
        let module_path = test_module_segments_for_file(project_root, source_root, path);
        let ast = parse_and_desugar_test_sources(
            &[(path.clone(), source.clone())],
            library_manifest_index,
            library_imported_vocab,
            library_imported_dsl_surfaces,
            compilation_session,
        )?;
        batch_files.insert(canonical_path_for_cache_key(path));
        parsed_sources.push((path.clone(), source.clone(), module_path, ast));
    }

    let mut deferred_dependencies = Vec::new();
    for (path, source, module_path, ast) in parsed_sources {
        let mut module_sources =
            read_conftest_sources(conftest_files_by_file.get(&path).map(Vec::as_slice).unwrap_or(&[]))?;
        module_sources.push((path.clone(), source.clone()));
        let combined_ast = if module_sources.len() == 1 {
            ast
        } else {
            parse_and_desugar_test_sources(
                &module_sources,
                library_manifest_index,
                library_imported_vocab,
                library_imported_dsl_surfaces,
                compilation_session,
            )?
        };
        let (runner_ast, fixtures) = prepare_runner_program(&combined_ast, testing_marker_semantics)?;
        let module_name = module_name_for_segments(&module_path);
        let module_source = module_sources
            .iter()
            .map(|(_, source)| source.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        for dependency in collect_source_modules_for_test(
            &runner_ast,
            source_root,
            Some(library_imported_vocab),
            Some(library_imported_dsl_surfaces),
            Some(library_manifest_index),
            compilation_session.provider_plan.as_ref(),
        )? {
            deferred_dependencies.push(dependency);
        }

        if seen_module_paths.insert(module_path.clone()) {
            source_modules.push(ParsedModule {
                name: module_name,
                path_segments: module_path.clone(),
                file_path: path.clone(),
                source: module_source,
                ast: runner_ast,
            });
        }
        harnesses.push(PreparedModuleHarness {
            file_path: path,
            module_path,
            fixtures,
        });
    }

    for dependency in deferred_dependencies {
        if batch_files.contains(&canonical_path_for_cache_key(&dependency.file_path)) {
            continue;
        }
        if seen_module_paths.insert(dependency.path_segments.clone()) {
            source_modules.push(dependency);
        }
    }

    let first_path = sources_by_file
        .first()
        .map(|(path, _)| path.as_path())
        .unwrap_or_else(|| Path::new("."));
    Ok(Some(IsolatedSourceModuleBatch {
        ast: empty_test_batch_root(first_path),
        source_modules,
        harnesses,
    }))
}

/// Resolve a dotted expression path using local import aliases collected from the runner AST.
fn resolved_expr_path(expr: &Spanned<Expr>, aliases: &HashMap<String, Vec<String>>) -> Option<Vec<String>> {
    match &expr.node {
        Expr::Ident(name) => aliases.get(name).cloned().or_else(|| Some(vec![name.clone()])),
        Expr::Field(base, field) => {
            let mut path = resolved_expr_path(base, aliases)?;
            path.push(field.clone());
            Some(path)
        }
        _ => None,
    }
}

/// Return the condition from a runner-only one-argument `std.testing.assert(...)` call statement.
fn runner_assert_condition(expr: &Spanned<Expr>, aliases: &HashMap<String, Vec<String>>) -> Option<Spanned<Expr>> {
    let (path, args) = match &expr.node {
        Expr::Call(callee, type_args, args) if type_args.is_empty() => (resolved_expr_path(callee, aliases)?, args),
        Expr::MethodCall(base, method, type_args, args) if type_args.is_empty() => {
            let mut path = resolved_expr_path(base, aliases)?;
            path.push(method.clone());
            (path, args)
        }
        _ => return None,
    };
    if args.len() != 1 {
        return None;
    }
    if path.as_slice() != ["std", "testing", "assert"] {
        return None;
    }
    let CallArg::Positional(condition) = &args[0] else {
        return None;
    };
    Some(condition.clone())
}

/// Rewrite `std.testing.assert(condition)` expression statements in a statement body to native assert statements.
fn normalize_runner_assert_statements_in_body(
    body: &mut Vec<Spanned<Statement>>,
    aliases: &HashMap<String, Vec<String>>,
) {
    for stmt in body {
        match &mut stmt.node {
            Statement::Expr(expr) => {
                if let Some(condition) = runner_assert_condition(expr, aliases) {
                    stmt.node = Statement::Assert(AssertStmt {
                        kind: AssertKind::Condition(condition),
                        message: None,
                    });
                }
            }
            Statement::If(if_stmt) => {
                normalize_runner_assert_statements_in_body(&mut if_stmt.then_body, aliases);
                for (_, body) in &mut if_stmt.elif_branches {
                    normalize_runner_assert_statements_in_body(body, aliases);
                }
                if let Some(body) = &mut if_stmt.else_body {
                    normalize_runner_assert_statements_in_body(body, aliases);
                }
            }
            Statement::Loop(loop_stmt) => normalize_runner_assert_statements_in_body(&mut loop_stmt.body, aliases),
            Statement::While(while_stmt) => normalize_runner_assert_statements_in_body(&mut while_stmt.body, aliases),
            Statement::For(for_stmt) => normalize_runner_assert_statements_in_body(&mut for_stmt.body, aliases),
            _ => {}
        }
    }
}

/// Normalize runner assertion helper call statements before lowering/codegen.
fn normalize_runner_assert_statements(ast: &mut Program) {
    let aliases = decorator_resolution::collect_import_aliases(ast);
    for decl in &mut ast.declarations {
        if let Declaration::Function(func) = &mut decl.node {
            normalize_runner_assert_statements_in_body(&mut func.body, &aliases);
        }
    }
}

/// Runner harness metadata for one source file emitted as its own Rust module.
pub(super) struct PreparedModuleHarness {
    pub file_path: PathBuf,
    pub module_path: Vec<String>,
    pub fixtures: HashMap<String, FixtureExecutionInfo>,
}

/// Return the generated function name that contains the post-yield teardown body.
/// Return the generated function name that contains the post-yield teardown body.
fn yield_fixture_teardown_name(name: &str) -> String {
    format!("__incan_fixture_teardown_{}", safe_fixture_ident(name))
}

#[derive(Debug, Clone)]
pub(super) struct YieldFixtureCapture {
    name: String,
    ty: Type,
}

#[derive(Debug, Clone)]
pub(super) struct YieldFixtureTeardown {
    teardown_function: String,
    captures: Vec<YieldFixtureCapture>,
    value_ty: Type,
}

/// Infer primitive fixture-capture types from literal setup assignments when no explicit annotation is present.
fn literal_type(expr: &Spanned<Expr>) -> Option<Type> {
    match &expr.node {
        Expr::Literal(crate::frontend::ast::Literal::Int(_)) => Some(Type::Simple("int".to_string())),
        Expr::Literal(crate::frontend::ast::Literal::Float(_)) => Some(Type::Simple("float".to_string())),
        Expr::Literal(crate::frontend::ast::Literal::Bool(_)) => Some(Type::Simple("bool".to_string())),
        Expr::Literal(crate::frontend::ast::Literal::String(_)) => Some(Type::Simple("str".to_string())),
        Expr::Literal(crate::frontend::ast::Literal::None) => Some(Type::Unit),
        _ => None,
    }
}

/// Return whether an expression reads a setup binding that must be preserved for yield teardown.
fn expr_references_name(expr: &Expr, name: &str) -> bool {
    match expr {
        Expr::Ident(ident) => ident == name,
        Expr::Unary(_, inner) | Expr::Try(inner) | Expr::Paren(inner) | Expr::Yield(Some(inner)) => {
            expr_references_name(&inner.node, name)
        }
        Expr::Binary(left, _, right) => {
            expr_references_name(&left.node, name) || expr_references_name(&right.node, name)
        }
        Expr::Call(callee, _, args) => {
            expr_references_name(&callee.node, name)
                || args.iter().any(|arg| match arg {
                    CallArg::Positional(expr)
                    | CallArg::Named(_, expr)
                    | CallArg::PositionalUnpack(expr)
                    | CallArg::KeywordUnpack(expr) => expr_references_name(&expr.node, name),
                })
        }
        Expr::Index(base, index) => expr_references_name(&base.node, name) || expr_references_name(&index.node, name),
        Expr::Slice(base, slice) => {
            expr_references_name(&base.node, name)
                || slice
                    .start
                    .as_ref()
                    .is_some_and(|expr| expr_references_name(&expr.node, name))
                || slice
                    .end
                    .as_ref()
                    .is_some_and(|expr| expr_references_name(&expr.node, name))
                || slice
                    .step
                    .as_ref()
                    .is_some_and(|expr| expr_references_name(&expr.node, name))
        }
        Expr::Field(base, _) => expr_references_name(&base.node, name),
        Expr::MethodCall(base, _, _, args) => {
            expr_references_name(&base.node, name)
                || args.iter().any(|arg| match arg {
                    CallArg::Positional(expr)
                    | CallArg::Named(_, expr)
                    | CallArg::PositionalUnpack(expr)
                    | CallArg::KeywordUnpack(expr) => expr_references_name(&expr.node, name),
                })
        }
        Expr::Match(scrutinee, arms) => {
            expr_references_name(&scrutinee.node, name)
                || arms.iter().any(|arm| match &arm.node.body {
                    crate::frontend::ast::MatchBody::Expr(expr) => expr_references_name(&expr.node, name),
                    crate::frontend::ast::MatchBody::Block(body) => body_references_name(body, name),
                })
        }
        Expr::If(if_expr) => {
            expr_references_name(&if_expr.condition.node, name)
                || body_references_name(&if_expr.then_body, name)
                || if_expr
                    .else_body
                    .as_ref()
                    .is_some_and(|body| body_references_name(body, name))
        }
        Expr::Loop(loop_expr) => body_references_name(&loop_expr.body, name),
        Expr::Generator(generator) => {
            expr_references_name(&generator.expr.node, name)
                || generator.clauses.iter().any(|clause| match clause {
                    crate::frontend::ast::ComprehensionClause::For { iter, .. } => {
                        expr_references_name(&iter.node, name)
                    }
                    crate::frontend::ast::ComprehensionClause::If(condition) => {
                        expr_references_name(&condition.node, name)
                    }
                })
        }
        Expr::ListComp(comp) => {
            expr_references_name(&comp.expr.node, name)
                || expr_references_name(&comp.iter.node, name)
                || comp
                    .filter
                    .as_ref()
                    .is_some_and(|expr| expr_references_name(&expr.node, name))
        }
        Expr::DictComp(comp) => {
            expr_references_name(&comp.key.node, name)
                || expr_references_name(&comp.value.node, name)
                || expr_references_name(&comp.iter.node, name)
                || comp
                    .filter
                    .as_ref()
                    .is_some_and(|expr| expr_references_name(&expr.node, name))
        }
        Expr::Closure(_, body) => expr_references_name(&body.node, name),
        Expr::Tuple(items) | Expr::Set(items) => items.iter().any(|item| expr_references_name(&item.node, name)),
        Expr::List(items) => items.iter().any(|item| match item {
            ListEntry::Element(value) | ListEntry::Spread(value) => expr_references_name(&value.node, name),
        }),
        Expr::Dict(pairs) => pairs.iter().any(|entry| match entry {
            DictEntry::Pair(key, value) => {
                expr_references_name(&key.node, name) || expr_references_name(&value.node, name)
            }
            DictEntry::Spread(value) => expr_references_name(&value.node, name),
        }),
        Expr::Constructor(_, args) => args.iter().any(|arg| match arg {
            CallArg::Positional(expr)
            | CallArg::Named(_, expr)
            | CallArg::PositionalUnpack(expr)
            | CallArg::KeywordUnpack(expr) => expr_references_name(&expr.node, name),
        }),
        Expr::FString(parts) => parts.iter().any(|part| {
            if let crate::frontend::ast::FStringPart::Expr { expr, .. } = part {
                expr_references_name(&expr.node, name)
            } else {
                false
            }
        }),
        Expr::Range { start, end, .. } => {
            expr_references_name(&start.node, name) || expr_references_name(&end.node, name)
        }
        Expr::VocabBlock(block) => {
            block
                .header_args
                .iter()
                .any(|arg| expr_references_name(&arg.node, name))
                || body_references_name(&block.body, name)
        }
        Expr::Literal(_) | Expr::SelfExpr | Expr::Yield(None) | Expr::Partial(_) | Expr::Surface(_) => false,
    }
}

/// Return whether a statement reads a setup binding that must be preserved for yield teardown.
fn statement_references_name(stmt: &Statement, name: &str) -> bool {
    match stmt {
        Statement::Assignment(assign) => expr_references_name(&assign.value.node, name),
        Statement::FieldAssignment(assign) => {
            expr_references_name(&assign.object.node, name) || expr_references_name(&assign.value.node, name)
        }
        Statement::IndexAssignment(assign) => {
            expr_references_name(&assign.object.node, name)
                || expr_references_name(&assign.index.node, name)
                || expr_references_name(&assign.value.node, name)
        }
        Statement::Return(Some(expr)) | Statement::Expr(expr) => expr_references_name(&expr.node, name),
        Statement::If(if_stmt) => {
            (match &if_stmt.condition {
                crate::frontend::ast::Condition::Expr(expr) => expr_references_name(&expr.node, name),
                crate::frontend::ast::Condition::Let { value, .. } => expr_references_name(&value.node, name),
            }) || body_references_name(&if_stmt.then_body, name)
                || if_stmt
                    .elif_branches
                    .iter()
                    .any(|(expr, body)| expr_references_name(&expr.node, name) || body_references_name(body, name))
                || if_stmt
                    .else_body
                    .as_ref()
                    .is_some_and(|body| body_references_name(body, name))
        }
        Statement::Loop(loop_stmt) => body_references_name(&loop_stmt.body, name),
        Statement::While(while_stmt) => {
            (match &while_stmt.condition {
                crate::frontend::ast::Condition::Expr(expr) => expr_references_name(&expr.node, name),
                crate::frontend::ast::Condition::Let { value, .. } => expr_references_name(&value.node, name),
            }) || body_references_name(&while_stmt.body, name)
        }
        Statement::For(for_stmt) => {
            expr_references_name(&for_stmt.iter.node, name) || body_references_name(&for_stmt.body, name)
        }
        Statement::Unsafe(unsafe_stmt) => body_references_name(&unsafe_stmt.body, name),
        Statement::Assert(assert_stmt) => match &assert_stmt.kind {
            AssertKind::Condition(expr) => expr_references_name(&expr.node, name),
            AssertKind::Raises { call, .. } => expr_references_name(&call.node, name),
            AssertKind::IsPattern { value, .. } => expr_references_name(&value.node, name),
        },
        Statement::CompoundAssignment(assign) => expr_references_name(&assign.value.node, name),
        Statement::TupleUnpack(assign) => expr_references_name(&assign.value.node, name),
        Statement::TupleAssign(assign) => {
            assign
                .targets
                .iter()
                .any(|target| expr_references_name(&target.node, name))
                || expr_references_name(&assign.value.node, name)
        }
        Statement::ChainedAssignment(assign) => expr_references_name(&assign.value.node, name),
        Statement::Return(None) | Statement::Pass | Statement::Break(None) | Statement::Continue => false,
        Statement::Break(Some(expr)) => expr_references_name(&expr.node, name),
        Statement::VocabExpressionItem(item) => {
            expr_references_name(&item.expr.node, name)
                || item
                    .modifiers
                    .iter()
                    .any(|modifier| expr_references_name(&modifier.value.node, name))
        }
        Statement::Surface(_) | Statement::VocabBlock(_) => false,
    }
}

/// Return whether any statement in a body reads a setup binding that must be preserved for yield teardown.
fn body_references_name(body: &[Spanned<Statement>], name: &str) -> bool {
    body.iter().any(|stmt| statement_references_name(&stmt.node, name))
}

/// Collect fixture parameters and typed setup locals that can be captured into generated teardown state.
fn capture_candidates(
    func: &crate::frontend::ast::FunctionDecl,
    setup_body: &[Spanned<Statement>],
) -> Vec<YieldFixtureCapture> {
    let mut captures = func
        .params
        .iter()
        .map(|param| YieldFixtureCapture {
            name: param.node.name.clone(),
            ty: param.node.ty.node.clone(),
        })
        .collect::<Vec<_>>();

    for stmt in setup_body {
        if let Statement::Assignment(assign) = &stmt.node {
            let ty = assign
                .ty
                .as_ref()
                .map(|ty| ty.node.clone())
                .or_else(|| literal_type(&assign.value));
            if let Some(ty) = ty {
                captures.push(YieldFixtureCapture {
                    name: assign.name.clone(),
                    ty,
                });
            }
        }
    }
    captures
}

/// Split runner fixture functions with a top-level `yield` into setup and teardown functions.
///
/// Incan does not lower general generators yet. For runner fixtures, RFC019 only needs the pytest-style boundary:
/// statements before `yield` produce the fixture value, and statements after `yield` are teardown. This transform keeps
/// that boundary runner-local and leaves production lowering untouched.
fn split_yield_fixture_declarations(
    ast: &mut Program,
    semantics: &TestingMarkerSemantics,
) -> Result<HashMap<String, YieldFixtureTeardown>, String> {
    let aliases = decorator_resolution::collect_import_aliases(ast);
    let mut teardowns = HashMap::new();
    let mut additional = Vec::new();

    for decl in &mut ast.declarations {
        let Declaration::Function(func) = &mut decl.node else {
            continue;
        };
        if !has_fixture_decorator(&func.decorators, &aliases, semantics) {
            continue;
        }
        let Some((yield_index, yielded)) = func.body.iter().enumerate().find_map(|(index, stmt)| {
            if let Statement::Expr(expr) = &stmt.node
                && let Expr::Yield(value) = &expr.node
            {
                Some((index, value.as_ref().map(|value| (**value).clone())))
            } else {
                None
            }
        }) else {
            continue;
        };

        let Some(yielded) = yielded else {
            return Err(format!(
                "fixture `{}` uses `yield` teardown without a yielded value; runner fixtures must yield the fixture value",
                func.name
            ));
        };
        let teardown_name = yield_fixture_teardown_name(&func.name);
        let mut setup_body = func.body[..yield_index].to_vec();
        let teardown_body = if yield_index + 1 < func.body.len() {
            func.body[yield_index + 1..].to_vec()
        } else {
            vec![Spanned::new(Statement::Pass, func.body[yield_index].span)]
        };
        let captures = capture_candidates(func, &setup_body)
            .into_iter()
            .filter(|capture| body_references_name(&teardown_body, &capture.name))
            .collect::<Vec<_>>();
        if let Some(name) = captures
            .iter()
            .filter(|capture| rust_type_for_fixture_cache(&capture.ty).is_none())
            .map(|capture| capture.name.as_str())
            .next()
        {
            return Err(format!(
                "fixture `{}` uses `yield` teardown with captured setup local `{name}` whose type cannot be stored by the runner; add an explicit primitive or tuple type annotation",
                func.name
            ));
        }

        if captures.is_empty() {
            setup_body.push(Spanned::new(
                Statement::Return(Some(yielded)),
                func.body[yield_index].span,
            ));
        } else {
            let mut tuple_items = Vec::with_capacity(1 + captures.len());
            tuple_items.push(yielded);
            tuple_items.extend(
                captures
                    .iter()
                    .map(|capture| Spanned::new(Expr::Ident(capture.name.clone()), func.body[yield_index].span)),
            );
            setup_body.push(Spanned::new(
                Statement::Return(Some(Spanned::new(
                    Expr::Tuple(tuple_items),
                    func.body[yield_index].span,
                ))),
                func.body[yield_index].span,
            ));
        }

        let mut teardown_func = func.clone();
        teardown_func.decorators.clear();
        teardown_func.name = teardown_name.clone();
        teardown_func.params = captures
            .iter()
            .map(|capture| {
                Spanned::new(
                    crate::frontend::ast::Param {
                        is_mut: false,
                        name: capture.name.clone(),
                        ty: Spanned::new(capture.ty.clone(), func.body[yield_index].span),
                        kind: ParamKind::Normal,
                        default: None,
                    },
                    func.body[yield_index].span,
                )
            })
            .collect();
        teardown_func.return_type.node = Type::Unit;
        teardown_func.body = teardown_body;
        let original_return_type = func.return_type.node.clone();
        if !captures.is_empty() {
            let mut state_types = Vec::with_capacity(1 + captures.len());
            state_types.push(Spanned::new(original_return_type.clone(), func.return_type.span));
            state_types.extend(
                captures
                    .iter()
                    .map(|capture| Spanned::new(capture.ty.clone(), func.return_type.span)),
            );
            func.return_type.node = Type::Tuple(state_types);
        }
        func.decorators.clear();
        func.body = setup_body;
        teardowns.insert(
            func.name.clone(),
            YieldFixtureTeardown {
                teardown_function: teardown_name,
                captures,
                value_ty: original_return_type,
            },
        );
        additional.push(Spanned::new(Declaration::Function(teardown_func), decl.span));
    }

    ast.declarations.extend(additional);
    Ok(teardowns)
}

fn canonical_path_for_cache_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn absolute_project_root(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };
    fs::canonicalize(&absolute).unwrap_or(absolute)
}

/// Infer a package root for manifest-less test runs.
///
/// Prefer conventional package anchors like `tests/` or `src/` so a file such as
/// `/repo/tests/test_cwd.incn` resolves its runtime cwd to `/repo`, not `/repo/tests`.
/// An unanchored file owns its containing directory: inheriting an enclosing caller
/// cwd would turn implementation paths such as `target/ci-nonroot/tmp` into Rust
/// module segments, where `ci-nonroot` is not a valid identifier.
fn infer_project_root_without_manifest(test_path: &Path) -> PathBuf {
    let absolute_test_path = if test_path.is_absolute() {
        test_path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(test_path)
    } else {
        test_path.to_path_buf()
    };
    let absolute_test_path = fs::canonicalize(&absolute_test_path).unwrap_or(absolute_test_path);

    for ancestor in absolute_test_path.ancestors().skip(1) {
        if ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "tests" | "src"))
            && let Some(parent) = ancestor.parent()
        {
            return parent.to_path_buf();
        }
    }

    absolute_test_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

/// Promote project dev dependencies into direct-rustc test-harness dependencies.
///
/// Generated user/test code lives in the caller-owned source tree, so every imported dependency belongs in the
/// explicit Oven build unit rather than in a Cargo-only development-dependency channel.
fn merge_test_runner_dependencies(
    dependencies: &[crate::manifest::DependencySpec],
    dev_dependencies: &[crate::manifest::DependencySpec],
) -> Result<Vec<crate::manifest::DependencySpec>, String> {
    let mut merged = dependencies.to_vec();
    for candidate in dev_dependencies {
        if let Some(existing) = merged.iter().find(|dep| dep.crate_name == candidate.crate_name) {
            if existing != candidate {
                return Err(format!(
                    "test runner dependency `{}` conflicts between dependencies and dev-dependencies",
                    candidate.crate_name
                ));
            }
            continue;
        }
        merged.push(candidate.clone());
    }
    merged.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    Ok(merged)
}

/// Build a stable generated-crate suffix for one worker batch, which may contain multiple source files.
fn file_batch_dir_suffix(file_paths: &[PathBuf], project_root: &Path) -> String {
    let mut hasher = Sha256::new();
    let mut paths = file_paths.to_vec();
    paths.sort();
    paths.dedup();
    let canonical_root = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    for file_path in paths {
        let canonical = fs::canonicalize(&file_path).unwrap_or(file_path);
        let logical_path = canonical.strip_prefix(&canonical_root).unwrap_or(&canonical);
        hasher.update(logical_path.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        match fs::read(&canonical) {
            Ok(source) => hasher.update(source),
            Err(_) => {
                // Missing collection inputs will fail later with a source diagnostic. Preserve a collision-safe suffix
                // for that failure path without making temporary project roots part of successful identities.
                hasher.update(b"missing-source\0");
                hasher.update(canonical.to_string_lossy().as_bytes());
            }
        }
        hasher.update(b"\0");
    }
    let digest = hex::encode(hasher.finalize());
    format!("batch_{}", &digest[..16])
}

/// Stable Rust crate name for one generated per-file test runner crate.
///
/// We derive this from the per-file batch suffix so shared `CARGO_TARGET_DIR` reuse does not alias crate identities
/// across different `.incn` files.
fn runner_crate_name_for_batch_suffix(batch_suffix: &str) -> String {
    let normalized = batch_suffix
        .strip_prefix("batch_")
        .unwrap_or(batch_suffix)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>();
    format!("test_runner_{}", normalized)
}

/// Name the caller-owned native output for one exact test selection.
///
/// A single source file can be split into several execution groups (for example, normal and expected-failure
/// cases). They share the generated-project directory but emit different harness sources. Giving every selection
/// its own output prevents one group from overwriting another group's receipt-verified native binary, which is
/// necessary for a subsequent unchanged `incan test` to reuse all of its outputs.
fn native_test_output_name(runner_crate_name: &str, tests: &[TestInfo]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"incan-oven-native-test-output/v1\0");
    for (index, test) in tests.iter().enumerate() {
        hasher.update(libtest_qualified_name(&harness_fn_name(test, index)).as_bytes());
        hasher.update(b"\0");
    }
    let digest = hex::encode(hasher.finalize());
    format!("{runner_crate_name}-{}-tests", &digest[..16])
}

/// Normalize a libtest test name by stripping any leading crate/module qualifiers before
/// [`INCAN_FILE_TEST_MOD`].
///
/// Examples:
/// - `__incan_file_tests::incan_harness_0_case` (unchanged)
/// - `test_runner::__incan_file_tests::incan_harness_0_case` (crate prefix removed)
fn normalize_libtest_test_name(name: &str) -> String {
    let trimmed = name.trim();
    if let Some(pos) = trimmed.find(INCAN_FILE_TEST_MOD) {
        trimmed[pos..].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Stable `#[test]` function name inside [`INCAN_FILE_TEST_MOD`] (indexed for guaranteed uniqueness).
fn harness_fn_name(test: &TestInfo, index: usize) -> String {
    let raw = test
        .parametrize_call
        .as_ref()
        .map(|p| p.display_id.as_str())
        .unwrap_or_else(|| test.function_name.as_str());
    let mut slug: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    while slug.contains("__") {
        slug = slug.replace("__", "_");
    }
    slug = slug.trim_matches('_').to_string();
    if slug.is_empty() {
        slug = "unnamed".to_string();
    }
    if slug.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        slug = format!("case_{slug}");
    }
    format!("incan_harness_{index}_{slug}")
}

/// Render a Rust string literal suitable for generated harness code.
fn rust_string_literal(value: &str) -> String {
    format!("{value:?}")
}

/// Generate setup and argument expression for a built-in fixture.
fn builtin_fixture_arg(
    name: &str,
    index: usize,
    setup: &mut String,
    created_builtins: &mut HashSet<String>,
) -> Option<String> {
    let safe_name: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let ident = format!("__incan_fixture_{index}_{safe_name}");
    match name {
        "tmp_path" => {
            if created_builtins.insert(name.to_string()) {
                setup.push_str(&format!(
                    "        let {ident} = std::env::temp_dir().join(format!(\"incan-test-{{}}-{index}-tmp-path\", std::process::id()));\n"
                ));
                setup.push_str(&format!(
                    "        if let Err(err) = std::fs::create_dir_all(&{ident}) {{ panic!(\"failed to create tmp_path fixture: {{}}\", err); }}\n"
                ));
            }
            Some(format!("{ident}.clone()"))
        }
        "tmp_workdir" => {
            if created_builtins.insert(name.to_string()) {
                setup.push_str(&format!(
                    "        let {ident} = std::env::temp_dir().join(format!(\"incan-test-{{}}-{index}-tmp-workdir\", std::process::id()));\n"
                ));
                setup.push_str(&format!(
                    "        if let Err(err) = std::fs::create_dir_all(&{ident}) {{ panic!(\"failed to create tmp_workdir fixture: {{}}\", err); }}\n"
                ));
                setup.push_str(&format!(
                    "        if let Err(err) = std::env::set_current_dir(&{ident}) {{ panic!(\"failed to enter tmp_workdir fixture: {{}}\", err); }}\n"
                ));
            }
            Some(format!("{ident}.clone()"))
        }
        "env" => {
            if created_builtins.insert(name.to_string()) {
                setup.push_str(&format!(
                    "        let mut {ident} = incan_stdlib::testing::TestEnv::new();\n"
                ));
            }
            Some(format!("&mut {ident}"))
        }
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub(super) struct FixtureExecutionInfo {
    params: Vec<String>,
    scope: FixtureScope,
    has_teardown: bool,
    is_async: bool,
    return_rust_type: Option<String>,
    state_rust_type: Option<String>,
    teardown: Option<YieldFixtureTeardown>,
}

/// Collect private items called by the generated Rust test harness.
fn collect_harness_entrypoints(
    tests: &[TestInfo],
    fixtures: &HashMap<String, FixtureExecutionInfo>,
) -> HashSet<String> {
    let mut entrypoints = HashSet::new();
    for test in tests {
        entrypoints.insert(test.function_name.clone());
        for fixture in &test.required_fixtures {
            collect_fixture_entrypoints(fixture, fixtures, &mut entrypoints, &mut Vec::new());
        }
    }
    entrypoints
}

/// Recursively collect fixture setup/teardown functions used by generated harness calls.
fn collect_fixture_entrypoints(
    name: &str,
    fixtures: &HashMap<String, FixtureExecutionInfo>,
    entrypoints: &mut HashSet<String>,
    visiting: &mut Vec<String>,
) {
    if visiting.iter().any(|existing| existing == name) {
        return;
    }
    let Some(fixture) = fixtures.get(name) else {
        return;
    };
    entrypoints.insert(name.to_string());
    if let Some(teardown) = &fixture.teardown {
        entrypoints.insert(teardown.teardown_function.clone());
    }
    visiting.push(name.to_string());
    for param in &fixture.params {
        collect_fixture_entrypoints(param, fixtures, entrypoints, visiting);
    }
    let _ = visiting.pop();
}

/// Convert a fixture name into an identifier fragment suitable for generated Rust.
fn safe_fixture_ident(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Return the shared generated cache static name for a broader-scope fixture.
fn fixture_cache_static_name(name: &str) -> String {
    let safe_name = safe_fixture_ident(name);
    format!("__INCAN_FIXTURE_CACHE_{}", safe_name.to_ascii_uppercase())
}

/// Return the Rust path used to access one generated fixture cache static.
fn fixture_cache_static_ref(name: &str, fixture: &FixtureExecutionInfo, session_cache_module: Option<&str>) -> String {
    let static_name = fixture_cache_static_name(name);
    if fixture.scope == FixtureScope::Session
        && let Some(module) = session_cache_module
    {
        return format!("crate::{module}::{static_name}");
    }
    static_name
}

/// Render one generated cache static for a module- or session-scoped fixture.
fn render_fixture_cache_static(name: &str, fixture: &FixtureExecutionInfo, visibility: &str) -> Option<String> {
    if fixture.scope == FixtureScope::Function {
        return None;
    }
    let static_name = fixture_cache_static_name(name);
    if fixture.has_teardown {
        let state_rust_type = fixture.state_rust_type.as_ref()?;
        return Some(format!(
            "{visibility}static {static_name}: std::sync::OnceLock<std::sync::Mutex<Option<{state_rust_type}>>> = std::sync::OnceLock::new();\n"
        ));
    }
    fixture.return_rust_type.as_ref()?;
    Some(format!(
        "{visibility}static {static_name}: std::sync::OnceLock<std::sync::Mutex<Option<Box<dyn std::any::Any + Send>>>> = std::sync::OnceLock::new();\n"
    ))
}

/// Return the local generated Rust binding that stores one fixture's setup/teardown state.
fn fixture_state_ident(index: usize, name: &str) -> String {
    format!("__incan_fixture_state_{index}_{}", safe_fixture_ident(name))
}

/// Return the local generated Rust binding that is passed to the user test as the fixture value.
fn fixture_value_ident(index: usize, name: &str) -> String {
    format!("__incan_fixture_value_{index}_{}", safe_fixture_ident(name))
}

/// Wrap an async generated harness call in the shared runner runtime when needed.
fn maybe_await_harness_call(call: String, is_async: bool) -> String {
    if is_async {
        format!("__incan_async_block_on({call})")
    } else {
        call
    }
}

/// Return the generated Rust expression that sets up one fixture.
fn fixture_setup_call(name: &str, args: &str, fixture: &FixtureExecutionInfo) -> String {
    maybe_await_harness_call(format!("super::{name}({args})"), fixture.is_async)
}

/// Return the generated Rust statement that tears down one yield fixture.
fn fixture_teardown_call(fixture: &FixtureExecutionInfo, teardown_function: &str, args: &str) -> String {
    let call = if args.is_empty() {
        format!("super::{teardown_function}()")
    } else {
        format!("super::{teardown_function}({args})")
    };
    format!("{};", maybe_await_harness_call(call, fixture.is_async))
}

/// Return whether the generated harness needs to drive async tests or fixtures.
fn harness_needs_async_runtime(tests: &[TestInfo], fixtures: &HashMap<String, FixtureExecutionInfo>) -> bool {
    tests.iter().any(|test| test.is_async) || fixtures.values().any(|fixture| fixture.is_async)
}

/// Add the stdlib async feature when the generated harness itself needs the runtime.
fn test_runner_stdlib_features(
    base: &[String],
    tests: &[TestInfo],
    fixtures: &HashMap<String, FixtureExecutionInfo>,
) -> Vec<String> {
    let mut features = base.iter().cloned().collect::<BTreeSet<_>>();
    if harness_needs_async_runtime(tests, fixtures) {
        features.insert("async".to_string());
    }
    features.into_iter().collect()
}

/// Collect stdlib feature flags needed by a test batch.
fn test_runner_stdlib_features_for_batch(
    base: &[String],
    tests: &[TestInfo],
    fixtures: &HashMap<String, FixtureExecutionInfo>,
    module_harnesses: &[PreparedModuleHarness],
) -> Vec<String> {
    if module_harnesses.is_empty() {
        return test_runner_stdlib_features(base, tests, fixtures);
    }

    let mut features = base.iter().cloned().collect::<BTreeSet<_>>();
    if module_harnesses.iter().any(|harness| {
        let file_tests = tests
            .iter()
            .filter(|test| test.file_path == harness.file_path)
            .cloned()
            .collect::<Vec<_>>();
        harness_needs_async_runtime(&file_tests, &harness.fixtures)
    }) {
        features.insert("async".to_string());
    }
    features.into_iter().collect()
}

struct FixtureArgRender<'a> {
    setup: &'a mut String,
    fixtures: &'a HashMap<String, FixtureExecutionInfo>,
    created_builtins: &'a mut HashSet<String>,
    teardown_steps: &'a mut Vec<String>,
    session_cache_module: Option<&'a str>,
}

impl FixtureArgRender<'_> {
    /// Generate an expression that calls a fixture, recursively filling fixture dependencies.
    fn arg(&mut self, name: &str, index: usize, visiting: &mut Vec<String>) -> String {
        if let Some(expr) = builtin_fixture_arg(name, index, self.setup, self.created_builtins) {
            return expr;
        }

        if visiting.iter().any(|existing| existing == name) {
            return format!("super::{name}()");
        }
        visiting.push(name.to_string());
        let params = self
            .fixtures
            .get(name)
            .map(|fixture| fixture.params.clone())
            .unwrap_or_default();
        let args = params
            .iter()
            .map(|param| self.arg(param, index, visiting))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = visiting.pop();
        let Some(fixture) = self.fixtures.get(name).cloned() else {
            return format!("super::{name}({args})");
        };
        let setup_call = fixture_setup_call(name, &args, &fixture);
        let Some(return_rust_type) = fixture.return_rust_type.as_ref() else {
            return setup_call;
        };
        if fixture.has_teardown {
            let Some(teardown) = &fixture.teardown else {
                return setup_call;
            };
            if fixture.scope == FixtureScope::Function {
                let state_ident = fixture_state_ident(index, name);
                let value_ident = fixture_value_ident(index, name);
                self.setup
                    .push_str(&format!("        let {state_ident} = {setup_call};\n"));
                if teardown.captures.is_empty() {
                    self.setup
                        .push_str(&format!("        let {value_ident} = {state_ident};\n"));
                    self.teardown_steps
                        .push(fixture_teardown_call(&fixture, &teardown.teardown_function, ""));
                } else {
                    let capture_names = teardown
                        .captures
                        .iter()
                        .map(|capture| format!("__incan_fixture_capture_{}_{}", safe_fixture_ident(name), capture.name))
                        .collect::<Vec<_>>();
                    self.setup.push_str(&format!(
                        "        let ({value_ident}, {}) = {state_ident};\n",
                        capture_names.join(", ")
                    ));
                    self.teardown_steps.push(fixture_teardown_call(
                        &fixture,
                        &teardown.teardown_function,
                        &capture_names.join(", "),
                    ));
                }
                return value_ident;
            }
            let static_name = fixture_cache_static_ref(name, &fixture, self.session_cache_module);
            if teardown.captures.is_empty() {
                return format!(
                    "{{\n\
                         let __incan_cache = {static_name}.get_or_init(|| std::sync::Mutex::new(None));\n\
                         let Ok(mut __incan_guard) = __incan_cache.lock() else {{ panic!(\"fixture cache `{name}` is poisoned\"); }};\n\
                         if __incan_guard.is_none() {{ *__incan_guard = Some({setup_call}); }}\n\
                         let Some(__incan_value) = __incan_guard.as_ref() else {{ panic!(\"fixture cache `{name}` was not initialized\"); }};\n\
                         __incan_value.clone()\n\
                     }}"
                );
            }
            return format!(
                "{{\n\
                         let __incan_cache = {static_name}.get_or_init(|| std::sync::Mutex::new(None));\n\
                         let Ok(mut __incan_guard) = __incan_cache.lock() else {{ panic!(\"fixture cache `{name}` is poisoned\"); }};\n\
                         if __incan_guard.is_none() {{ *__incan_guard = Some({setup_call}); }}\n\
                         let Some(__incan_state) = __incan_guard.as_ref() else {{ panic!(\"fixture cache `{name}` was not initialized\"); }};\n\
                         let __incan_value: &{return_rust_type} = &__incan_state.0;\n\
                         __incan_value.clone()\n\
                     }}"
            );
        }
        if fixture.scope == FixtureScope::Function {
            return setup_call;
        }

        let static_name = fixture_cache_static_ref(name, &fixture, self.session_cache_module);
        format!(
            "{{\n\
                     let __incan_cache = {static_name}.get_or_init(|| std::sync::Mutex::new(None));\n\
                     let Ok(mut __incan_guard) = __incan_cache.lock() else {{ panic!(\"fixture cache `{name}` is poisoned\"); }};\n\
                     if __incan_guard.is_none() {{ *__incan_guard = Some(Box::new({setup_call})); }}\n\
                     let Some(__incan_boxed) = __incan_guard.as_ref() else {{ panic!(\"fixture cache `{name}` was not initialized\"); }};\n\
                     let Some(__incan_value) = __incan_boxed.downcast_ref::<{return_rust_type}>() else {{ panic!(\"fixture cache `{name}` had an unexpected type\"); }};\n\
                     __incan_value.clone()\n\
                 }}"
        )
    }
}

/// Generate the body statement that invokes one collected test case.
fn harness_call(
    test: &TestInfo,
    index: usize,
    fixtures: &HashMap<String, FixtureExecutionInfo>,
    session_cache_module: Option<&str>,
) -> String {
    let mut setup = String::new();
    let mut args = Vec::new();
    let mut teardown_steps = Vec::new();
    let mut used_fixtures = HashSet::new();
    let mut created_builtins = HashSet::new();
    let parametrize = test.parametrize_call.as_ref();

    {
        let mut fixture_render = FixtureArgRender {
            setup: &mut setup,
            fixtures,
            created_builtins: &mut created_builtins,
            teardown_steps: &mut teardown_steps,
            session_cache_module,
        };

        for param_name in &test.parameter_names {
            if let Some(call) = parametrize
                && let Some(pos) = call.argument_names.iter().position(|name| name == param_name)
                && let Some(value) = call.rust_arguments.get(pos)
            {
                args.push(value.clone());
                continue;
            }

            if test.required_fixtures.iter().any(|fixture| fixture == param_name) {
                used_fixtures.insert(param_name.clone());
                args.push(fixture_render.arg(param_name, index, &mut Vec::new()));
            }
        }

        if test.parameter_names.is_empty() {
            if let Some(call) = parametrize {
                args.extend(call.rust_arguments.clone());
            }
            for fixture in &test.required_fixtures {
                used_fixtures.insert(fixture.clone());
                args.push(fixture_render.arg(fixture, index, &mut Vec::new()));
            }
        }

        for fixture in &test.required_fixtures {
            if !used_fixtures.contains(fixture) {
                let expr = fixture_render.arg(fixture, index, &mut Vec::new());
                fixture_render.setup.push_str(&format!("        let _ = {expr};\n"));
            }
        }
    }

    let joined = args.join(", ");
    let test_call = maybe_await_harness_call(format!("super::{}({joined})", test.function_name), test.is_async);
    if teardown_steps.is_empty() {
        return format!("{setup}        {test_call};\n");
    }

    let mut teardown = String::new();
    for step in teardown_steps.iter().rev() {
        teardown.push_str("        __incan_run_teardown(&mut __incan_teardown_failures, || { ");
        teardown.push_str(step);
        teardown.push_str(" });\n");
    }
    format!(
        "{setup}        let mut __incan_teardown_failures = Vec::new();\n\
                 let __incan_test_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {{\n\
                     {test_call};\n\
                 }}));\n\
         {teardown}        if !__incan_teardown_failures.is_empty() {{ panic!(\"fixture teardown failed:\\n{{}}\", __incan_teardown_failures.join(\"\\n\")); }}\n\
                 if let Err(__incan_panic) = __incan_test_result {{ std::panic::resume_unwind(__incan_panic); }}\n",
    )
}

/// Map cacheable fixture return types to the Rust types stored by generated module/session fixture caches.
fn rust_type_for_fixture_cache(ty: &Type) -> Option<String> {
    match ty {
        Type::Simple(name) => match name.as_str() {
            "int" => Some("i64".to_string()),
            "float" => Some("f64".to_string()),
            "bool" => Some("bool".to_string()),
            "str" => Some("String".to_string()),
            _ => None,
        },
        Type::Unit => Some("()".to_string()),
        Type::Tuple(elements) => {
            let mut rendered = Vec::new();
            for element in elements {
                rendered.push(rust_type_for_fixture_cache(&element.node)?);
            }
            Some(format!("({})", rendered.join(", ")))
        }
        _ => None,
    }
}

/// Collect runner-only fixture lifecycle metadata used by generated harness calls.
fn collect_fixture_execution_info(
    ast: &crate::frontend::ast::Program,
    fixture_teardowns: &HashMap<String, YieldFixtureTeardown>,
    semantics: &TestingMarkerSemantics,
) -> HashMap<String, FixtureExecutionInfo> {
    let aliases = decorator_resolution::collect_import_aliases(ast);
    ast.declarations
        .iter()
        .filter_map(|decl| {
            if let crate::frontend::ast::Declaration::Function(func) = &decl.node {
                let is_fixture = func.decorators.iter().any(|decorator| {
                    resolve_testing_marker_kind(&decorator.node, &aliases, semantics)
                        == Some(TestingMarkerKind::Fixture)
                });
                if !is_fixture {
                    return None;
                }
                let mut scope = FixtureScope::Function;
                for decorator in &func.decorators {
                    if resolve_testing_marker_kind(&decorator.node, &aliases, semantics)
                        != Some(TestingMarkerKind::Fixture)
                    {
                        continue;
                    }
                    for arg in &decorator.node.args {
                        if let crate::frontend::ast::DecoratorArg::Named(name, value) = arg
                            && name == &semantics.fixture_scope_arg
                            && let crate::frontend::ast::DecoratorArgValue::Expr(expr) = value
                            && let Expr::Literal(crate::frontend::ast::Literal::String(value)) = &expr.node
                        {
                            scope = match value.as_str() {
                                value if value == semantics.fixture_scope_module.as_str() => FixtureScope::Module,
                                value if value == semantics.fixture_scope_session.as_str() => FixtureScope::Session,
                                _ => FixtureScope::Function,
                            };
                        }
                    }
                }
                let teardown = fixture_teardowns.get(&func.name).cloned();
                Some((
                    func.name.clone(),
                    FixtureExecutionInfo {
                        params: func.params.iter().map(|param| param.node.name.clone()).collect(),
                        scope,
                        has_teardown: teardown.is_some(),
                        is_async: func.is_async(),
                        return_rust_type: teardown
                            .as_ref()
                            .and_then(|teardown| rust_type_for_fixture_cache(&teardown.value_ty))
                            .or_else(|| rust_type_for_fixture_cache(&func.return_type.node)),
                        state_rust_type: rust_type_for_fixture_cache(&func.return_type.node),
                        teardown,
                    },
                ))
            } else {
                None
            }
        })
        .collect()
}

/// Attach runner-local teardown metadata to fixture declarations collected before yield-splitting.
fn apply_fixture_teardowns(
    fixtures: &mut HashMap<String, FixtureExecutionInfo>,
    fixture_teardowns: &HashMap<String, YieldFixtureTeardown>,
) {
    for (name, teardown) in fixture_teardowns {
        let Some(fixture) = fixtures.get_mut(name) else {
            continue;
        };
        fixture.has_teardown = true;
        fixture.return_rust_type = rust_type_for_fixture_cache(&teardown.value_ty);
        fixture.state_rust_type = if teardown.captures.is_empty() {
            rust_type_for_fixture_cache(&teardown.value_ty)
        } else {
            let mut state_types = Vec::with_capacity(1 + teardown.captures.len());
            state_types.push(Spanned::new(teardown.value_ty.clone(), Span::default()));
            state_types.extend(
                teardown
                    .captures
                    .iter()
                    .map(|capture| Spanned::new(capture.ty.clone(), Span::default())),
            );
            rust_type_for_fixture_cache(&Type::Tuple(state_types))
        };
        fixture.teardown = Some(teardown.clone());
    }
}

/// Append a `#[cfg(test)]` module with one `#[test]` per collected case so native libtest runs a compatible batch in
/// one process.
///
/// The generated harness resets the process cwd to the source project root before each test so fixture paths behave
/// the same way as ordinary `incan run/build/test` entrypoints rather than inheriting the generated temp crate path.
fn inject_file_test_harness(
    rust_code: &str,
    tests: &[TestInfo],
    project_root: &Path,
    fixtures: &HashMap<String, FixtureExecutionInfo>,
) -> String {
    let test_indices = (0..tests.len()).collect::<Vec<_>>();
    inject_file_test_harness_with_indices(rust_code, tests, &test_indices, project_root, fixtures, None)
}

/// Render the shared session-fixture cache module used by isolated multi-file test batches.
fn render_shared_session_fixture_cache_module(module_harnesses: &[PreparedModuleHarness]) -> Option<String> {
    let mut statics = BTreeSet::new();
    for harness in module_harnesses {
        for (name, fixture) in &harness.fixtures {
            if fixture.scope != FixtureScope::Session {
                continue;
            }
            if let Some(static_decl) = render_fixture_cache_static(name, fixture, "pub(crate) ") {
                statics.insert(static_decl);
            }
        }
    }
    if statics.is_empty() {
        return None;
    }
    Some(statics.into_iter().collect::<Vec<_>>().join(""))
}

/// Inject generated Rust test harness entries using stable test indices.
fn inject_file_test_harness_with_indices(
    rust_code: &str,
    tests: &[TestInfo],
    test_indices: &[usize],
    project_root: &Path,
    fixtures: &HashMap<String, FixtureExecutionInfo>,
    session_cache_module: Option<&str>,
) -> String {
    let mut out = rust_code.to_string();
    let project_root_literal = project_root.to_string_lossy().to_string();
    out.push_str("\n\n#[cfg(test)]\nmod ");
    out.push_str(INCAN_FILE_TEST_MOD);
    out.push_str(" {\n");
    for (name, fixture) in fixtures {
        if fixture.scope == FixtureScope::Function
            || (fixture.scope == FixtureScope::Session && session_cache_module.is_some())
        {
            continue;
        }
        if let Some(static_decl) = render_fixture_cache_static(name, fixture, "") {
            out.push_str(&static_decl);
        }
    }
    out.push_str(
        "struct __IncanCwdGuard(Option<std::path::PathBuf>);\n\
         impl Drop for __IncanCwdGuard {\n\
             /// Restore the cwd that was active before the generated test ran.\n\
             fn drop(&mut self) {\n\
                 if let Some(path) = self.0.as_ref() { let _ = std::env::set_current_dir(path); }\n\
             }\n\
         }\n",
    );
    if fixtures.values().any(|fixture| fixture.has_teardown) {
        out.push_str(
            "fn __incan_run_teardown<F>(failures: &mut Vec<String>, teardown: F)\n\
             where\n\
                 F: FnOnce(),\n\
             {\n\
                 if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(teardown)) {\n\
                     let message = if let Some(message) = payload.downcast_ref::<&str>() {\n\
                         (*message).to_string()\n\
                     } else if let Some(message) = payload.downcast_ref::<String>() {\n\
                         message.clone()\n\
                     } else {\n\
                         \"non-string panic payload\".to_string()\n\
                     };\n\
                     failures.push(message);\n\
                 }\n\
             }\n",
        );
    }
    if harness_needs_async_runtime(tests, fixtures) {
        out.push_str(
            "static __INCAN_ASYNC_RUNTIME: std::sync::OnceLock<incan_stdlib::__private::tokio::runtime::Runtime> = std::sync::OnceLock::new();\n\
             /// Drive one async generated test or fixture on the shared runner runtime.\n\
             fn __incan_async_block_on<F>(future: F) -> F::Output\n\
             where\n\
                 F: std::future::Future,\n\
             {\n\
                 let __incan_runtime = __INCAN_ASYNC_RUNTIME.get_or_init(|| {\n\
                     let mut builder = incan_stdlib::__private::tokio::runtime::Builder::new_multi_thread();\n\
                     builder.enable_all();\n\
                     match builder.build() {\n\
                         Ok(runtime) => runtime,\n\
                         Err(err) => panic!(\"failed to build async test runtime: {}\", err),\n\
                     }\n\
                 });\n\
                 __incan_runtime.block_on(future)\n\
             }\n",
        );
    }
    let teardown_fixtures = ordered_teardown_fixtures(tests, fixtures);
    for (index, t) in test_indices.iter().copied().zip(tests.iter()) {
        let fname = harness_fn_name(t, index);
        let call = harness_call(t, index, fixtures, session_cache_module);
        out.push_str("    #[test]\n    fn ");
        out.push_str(&fname);
        out.push_str("() {\n");
        out.push_str("        let __incan_cwd_guard = __IncanCwdGuard(std::env::current_dir().ok());\n");
        out.push_str("        let _ = &__incan_cwd_guard;\n");
        out.push_str("        if let Err(err) = std::env::set_current_dir(");
        out.push_str(&rust_string_literal(&project_root_literal));
        out.push_str(") {\n");
        out.push_str("            panic!(\"failed to set generated test cwd: {}\", err);\n");
        out.push_str("        }\n");
        out.push_str(&call);
        out.push_str("    }\n");
    }
    if !teardown_fixtures.is_empty() {
        out.push_str("    #[test]\n    fn zzzz_incan_harness_teardown_cached_fixtures() {\n");
        out.push_str("        let mut __incan_teardown_failures = Vec::new();\n");
        out.push_str("        let __incan_cwd_guard = __IncanCwdGuard(std::env::current_dir().ok());\n");
        out.push_str("        let _ = &__incan_cwd_guard;\n");
        out.push_str("        if let Err(err) = std::env::set_current_dir(");
        out.push_str(&rust_string_literal(&project_root_literal));
        out.push_str(") {\n");
        out.push_str("            panic!(\"failed to set generated test cwd: {}\", err);\n");
        out.push_str("        }\n");
        for name in teardown_fixtures.iter().rev() {
            let Some(fixture) = fixtures.get(name) else {
                continue;
            };
            let Some(teardown) = &fixture.teardown else {
                continue;
            };
            let static_name = fixture_cache_static_ref(name, fixture, session_cache_module);
            out.push_str(&format!(
                "        if let Some(__incan_cache) = {static_name}.get() {{\n\
                         let Ok(mut __incan_guard) = __incan_cache.lock() else {{ panic!(\"fixture cache `{name}` is poisoned\"); }};\n\
                         if let Some(__incan_state) = __incan_guard.take() {{\n"
            ));
            if teardown.captures.is_empty() {
                out.push_str("            let _ = __incan_state;\n");
                out.push_str(&format!(
                    "            __incan_run_teardown(&mut __incan_teardown_failures, || {{ {} }});\n",
                    fixture_teardown_call(fixture, &teardown.teardown_function, "")
                ));
            } else {
                let capture_names = teardown
                    .captures
                    .iter()
                    .map(|capture| format!("__incan_fixture_capture_{}_{}", safe_fixture_ident(name), capture.name))
                    .collect::<Vec<_>>();
                out.push_str(&format!(
                    "            let (_, {}) = __incan_state;\n",
                    capture_names.join(", ")
                ));
                out.push_str(&format!(
                    "            __incan_run_teardown(&mut __incan_teardown_failures, || {{ {} }});\n",
                    fixture_teardown_call(fixture, &teardown.teardown_function, &capture_names.join(", "))
                ));
            }
            out.push_str("                         }\n        }\n");
        }
        out.push_str(
            "        if !__incan_teardown_failures.is_empty() {\n\
                         panic!(\"fixture teardown failed:\\n{}\", __incan_teardown_failures.join(\"\\n\"));\n\
                     }\n",
        );
        out.push_str("    }\n");
    }
    out.push_str("}\n");
    out
}

/// Add a broader-scoped teardown fixture after its dependencies so reverse iteration tears dependents down first.
fn push_fixture_order(name: &str, fixtures: &HashMap<String, FixtureExecutionInfo>, ordered: &mut Vec<String>) {
    if ordered.iter().any(|existing| existing == name) {
        return;
    }
    let Some(fixture) = fixtures.get(name) else {
        return;
    };
    for dependency in &fixture.params {
        push_fixture_order(dependency, fixtures, ordered);
    }
    if fixture.scope != FixtureScope::Function && fixture.has_teardown {
        ordered.push(name.to_string());
    }
}

/// Return broader-scoped teardown fixtures used by a worker batch in setup dependency order.
fn ordered_teardown_fixtures(tests: &[TestInfo], fixtures: &HashMap<String, FixtureExecutionInfo>) -> Vec<String> {
    let mut ordered = Vec::new();
    for test in tests {
        for fixture in &test.required_fixtures {
            push_fixture_order(fixture, fixtures, &mut ordered);
        }
    }
    ordered
}

/// Parse libtest lines: `test <name> ... ok|FAILED`.
fn parse_libtest_outcomes(combined: &str) -> HashMap<String, bool> {
    let mut map = HashMap::new();
    let mut pending_name: Option<String> = None;
    for line in combined.lines() {
        let line = line.trim();
        if line == "ok" {
            if let Some(name) = pending_name.take() {
                map.insert(name, true);
            }
            continue;
        }
        let Some(rest) = line.strip_prefix("test ") else {
            continue;
        };
        let Some((name, tail)) = rest.split_once(" ... ") else {
            continue;
        };
        let status = tail.trim();
        let passed = status.starts_with("ok");
        let failed = status.starts_with("FAILED");
        if passed || failed {
            map.insert(normalize_libtest_test_name(name), passed);
        } else {
            pending_name = Some(normalize_libtest_test_name(name));
        }
    }
    map
}

/// Fully qualified Rust test name as libtest prints it for harness functions under [`INCAN_FILE_TEST_MOD`].
fn libtest_qualified_name(fn_name: &str) -> String {
    format!("{INCAN_FILE_TEST_MOD}::{fn_name}")
}

/// Best-effort extraction of failure output for one harness `fn_name` from combined native-test stdout/stderr.
///
/// Looks for libtest `---- <qualified> stdout ----` sections, then falls back to panic/assertion heuristics or the
/// full trimmed output.
fn extract_libtest_failure_detail(combined: &str, full_name: &str) -> String {
    for line in combined.lines() {
        let line = line.trim();
        if line.starts_with("---- ")
            && line.ends_with(" stdout ----")
            && normalize_libtest_test_name(line).contains(full_name)
            && let Some(pos) = combined.find(line)
        {
            let after = &combined[pos + line.len()..];
            let end = after
                .find("\n---- ")
                .unwrap_or_else(|| after.find("\nfailures:").unwrap_or(after.len()));
            let body = after[..end].trim();
            if !body.is_empty() {
                return body.to_string();
            }
        }
    }
    if combined.contains("panicked at") {
        return extract_panic_message(combined);
    }
    if combined.contains("assertion") {
        return extract_assertion_error(combined);
    }
    combined.trim().to_string()
}

/// Turn one batched native-test run into per-[`TestInfo`] results.
///
/// Individual outcomes come from [`parse_libtest_outcomes`]. A verified successful native batch makes any
/// unreported selected harness passed; wall time is split evenly across tests for display.
fn map_batch_results(
    tests: &[TestInfo],
    combined_output: &str,
    elapsed: std::time::Duration,
    native_batch_succeeded: bool,
    generated_root: &Path,
    crate_name: &str,
) -> Vec<(TestInfo, TestResult)> {
    let outcomes = parse_libtest_outcomes(combined_output);
    let per_test_ms = elapsed.as_millis() / tests.len().max(1) as u128;
    let batch_failed = !native_batch_succeeded
        && (combined_output.contains("test result: FAILED") || combined_output.contains("failures:"));
    let expected_failures = tests
        .iter()
        .enumerate()
        .filter(|(index, t)| {
            let fname = harness_fn_name(t, *index);
            let full = libtest_qualified_name(&fname);
            outcomes.get(&full) == Some(&false)
        })
        .count();
    let teardown_failure = batch_failed && expected_failures == 0;

    tests
        .iter()
        .enumerate()
        .map(|(index, t)| {
            let fname = harness_fn_name(t, index);
            let full = libtest_qualified_name(&fname);
            let result = match outcomes.get(&full) {
                Some(true) => TestResult::Passed(std::time::Duration::from_millis(per_test_ms as u64)),
                Some(false) => {
                    let detail = extract_libtest_failure_detail(combined_output, &full);
                    TestResult::Failed(std::time::Duration::from_millis(per_test_ms as u64), detail)
                }
                None if native_batch_succeeded => TestResult::Passed(std::time::Duration::from_millis(per_test_ms as u64)),
                None if teardown_failure && index + 1 == tests.len() => TestResult::Failed(
                    std::time::Duration::from_millis(per_test_ms as u64),
                    extract_libtest_failure_detail(combined_output, "zzzz_incan_harness_teardown_cached_fixtures"),
                ),
                None => TestResult::Failed(
                    elapsed,
                    if combined_output.contains(INCAN_FILE_TEST_MOD) {
                        format!(
                            "Test runner did not report outcome for `{full}`.\ngenerated-root=`{}` crate=`{}`\nThis may indicate stale caller-owned test output.\n{combined_output}",
                            generated_root.display(),
                            crate_name,
                        )
                    } else {
                        format!(
                            "Test runner did not report outcome for `{full}` (see native-test output below)\n{combined_output}"
                        )
                    },
                ),
            };
            (t.clone(), result)
        })
        .collect()
}

/// Run one collected test execution unit through the Oven Alpha direct-rustc path.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_file_tests_batch(
    tests: &[TestInfo],
    conftest_files_by_file: &HashMap<PathBuf, Vec<PathBuf>>,
    cargo_policy: &CargoPolicy,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    cargo_features: &[String],
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    options: TestExecutionOptions,
) -> Vec<(TestInfo, TestResult)> {
    run_file_tests_batch_oven(
        tests,
        conftest_files_by_file,
        cargo_policy,
        package_features,
        sdk_profile_override,
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
        options,
    )
}

/// Execute one Oven Alpha test unit without starting Cargo or reading a generated Cargo target directory.
///
/// The generated harness and receipt stay under the caller's project. Its source authorizes this one invocation, while
/// the compatibility identity selects a reusable store-owned closure across test files, workers, and clean worktrees.
#[allow(clippy::too_many_arguments)]
fn run_file_tests_batch_oven(
    tests: &[TestInfo],
    conftest_files_by_file: &HashMap<PathBuf, Vec<PathBuf>>,
    cargo_policy: &CargoPolicy,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    cargo_features: &[String],
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    options: TestExecutionOptions,
) -> Vec<(TestInfo, TestResult)> {
    if tests.is_empty() {
        return Vec::new();
    }

    let start = Instant::now();
    let frontend_start = Instant::now();
    let first = &tests[0];
    let failure = |message: String| {
        tests
            .iter()
            .map(|test| (test.clone(), TestResult::Failed(start.elapsed(), message.clone())))
            .collect::<Vec<_>>()
    };
    if !cargo_policy.extra_args.is_empty()
        || cargo_no_default_features
        || cargo_all_features
        || !cargo_features.is_empty()
    {
        return failure(
            "Oven Alpha normal test execution does not accept Cargo passthrough or feature controls; use Incan package features instead"
                .to_string(),
        );
    }
    let mut source_parts = Vec::new();
    let mut batch_parse_sources = Vec::new();
    let mut sources_by_file = Vec::new();
    let mut seen_conftests = BTreeSet::new();
    let mut seen_files = BTreeSet::new();
    for test in tests {
        if !seen_files.insert(test.file_path.clone()) {
            continue;
        }
        if let Some(conftests) = conftest_files_by_file.get(&test.file_path) {
            for conftest in conftests {
                if !seen_conftests.insert(conftest.clone()) {
                    continue;
                }
                match fs::read_to_string(conftest) {
                    Ok(source) => {
                        source_parts.push(source.clone());
                        batch_parse_sources.push((conftest.clone(), source));
                    }
                    Err(error) => return failure(format!("failed to read conftest {}: {error}", conftest.display())),
                }
            }
        }
        match fs::read_to_string(&test.file_path) {
            Ok(source) => {
                source_parts.push(source.clone());
                batch_parse_sources.push((test.file_path.clone(), source.clone()));
                sources_by_file.push((test.file_path.clone(), source));
            }
            Err(error) => return failure(format!("failed to read test {}: {error}", test.file_path.display())),
        }
    }
    let source = source_parts.join("\n");

    let session =
        match common::CompilationSession::discover_for_oven(&first.file_path, package_features, sdk_profile_override) {
            Ok(session) => session,
            Err(error) => return failure(error.message),
        };
    let manifest = session.manifest.clone();
    let project_root = absolute_project_root(
        &manifest
            .as_ref()
            .map(|manifest| manifest.project_root().to_path_buf())
            .unwrap_or_else(|| infer_project_root_without_manifest(&first.file_path)),
    );
    let library_manifest_index = session.library_manifest_index.clone();
    let library_imported_vocab = library_manifest_index.library_imported_vocab();
    let library_imported_dsl_surfaces = library_manifest_index.library_imported_dsl_surfaces();
    let testing_marker_semantics = match session.testing_marker_semantics() {
        Ok(semantics) => semantics,
        Err(error) => return failure(error.message),
    };
    let source_root = common::resolve_source_root(&project_root, manifest.as_ref());
    let isolated_batch = match prepare_isolated_source_module_batch(
        &sources_by_file,
        conftest_files_by_file,
        &project_root,
        &source_root,
        &library_manifest_index,
        &library_imported_vocab,
        &library_imported_dsl_surfaces,
        &session,
        testing_marker_semantics.as_ref(),
    ) {
        Ok(batch) => batch,
        Err(message) => return failure(message),
    };
    let (runner_ast, fixtures, source_modules, module_harnesses) = if let Some(batch) = isolated_batch {
        (batch.ast, HashMap::new(), batch.source_modules, batch.harnesses)
    } else {
        let parsed = match parse_and_desugar_test_sources(
            &batch_parse_sources,
            &library_manifest_index,
            &library_imported_vocab,
            &library_imported_dsl_surfaces,
            &session,
        ) {
            Ok(parsed) => parsed,
            Err(message) => return failure(message),
        };
        let (runner_ast, fixtures) = match prepare_runner_program(&parsed, testing_marker_semantics.as_ref()) {
            Ok(prepared) => prepared,
            Err(message) => return failure(message),
        };
        let source_modules = match collect_source_modules_for_test(
            &runner_ast,
            &source_root,
            Some(&library_imported_vocab),
            Some(&library_imported_dsl_surfaces),
            Some(&library_manifest_index),
            session.provider_plan.as_ref(),
        ) {
            Ok(modules) => modules,
            Err(error) => return failure(format!("failed to collect source modules: {error}")),
        };
        (runner_ast, fixtures, source_modules, Vec::new())
    };
    let module_for_imports = ParsedModule {
        name: "test".to_string(),
        path_segments: vec!["test".to_string()],
        file_path: first.file_path.clone(),
        source,
        ast: runner_ast.clone(),
    };
    let source_dependency_modules = source_modules
        .iter()
        .map(|module| ParsedModule {
            name: module.name.clone(),
            path_segments: module.path_segments.clone(),
            file_path: module.file_path.clone(),
            source: module.source.clone(),
            ast: module.ast.clone(),
        })
        .collect::<Vec<_>>();
    // `std.*` source modules lower through the compiler-owned `incan_stdlib` crate. ProjectGenerator always supplies
    // that runtime directly, so treating its internal `rust.module("incan_stdlib::...")` markers as user Cargo
    // imports would create a duplicate, path-unstable dependency. Every other inline Rust import remains part of the
    // explicit publisher's generated manifest and therefore its Oven build-unit identity.
    let inline_imports = collect_test_dependency_inline_imports(&module_for_imports, &source_dependency_modules)
        .into_iter()
        .filter(|import| import.crate_name != "incan_stdlib")
        .collect::<Vec<_>>();
    let mut dependency_modules = Vec::with_capacity(1 + source_dependency_modules.len());
    dependency_modules.push(module_for_imports.clone());
    dependency_modules.extend(source_dependency_modules.clone());
    let mut requirements = match common::collect_project_requirements(&dependency_modules, &library_manifest_index) {
        Ok(requirements) => requirements,
        Err(error) => return failure(error.message),
    };
    let feature_selection = CargoFeatureSelection::default().normalized();
    let mut resolved =
        match resolve_reachable_dependencies(manifest.as_ref(), &inline_imports, true, &feature_selection) {
            Ok(resolved) => resolved,
            Err(errors) => {
                let sources = common::build_source_map(&dependency_modules);
                return failure(
                    errors
                        .iter()
                        .map(|error| common::format_dependency_error(error, &sources))
                        .collect::<String>(),
                );
            }
        };
    let provider_plan = match session.provider_plan_for_modules(&dependency_modules) {
        Ok(plan) => plan,
        Err(error) => return failure(error.message),
    };
    // A compiled `pub::` package is a caller-owned direct-Rustc artifact. Keep its prior direct output as an explicit
    // precondition, then replace it below when a compiler-suite consumer selects a different complete Rustc cohort.
    // The immutable native plan remains responsible only for compiler-owned SDK/runtime inputs; no missing or stale
    // library authorizes a Cargo path.
    let mut caller_owned_libraries =
        match crate::cli::commands::build::oven_caller_owned_libraries(&provider_plan, "debug") {
            Ok(libraries) => libraries,
            Err(error) => return failure(error.message),
        };
    if let Err(error) = common::extend_requirements_with_provider_plan(&mut requirements, &provider_plan) {
        return failure(error.message);
    }
    if let Err(error) = common::merge_project_requirement_dependencies(&mut resolved, &requirements) {
        return failure(error.message);
    }
    let inline_path_dependencies = oven_test_inline_dependency_specs(&resolved, &inline_imports);
    let project_name = manifest
        .as_ref()
        .and_then(|manifest| manifest.project.as_ref().and_then(|project| project.name.clone()))
        .unwrap_or_else(|| "incan_test".to_string());
    let project_version = manifest
        .as_ref()
        .and_then(|manifest| manifest.project.as_ref().and_then(|project| project.version.clone()))
        .unwrap_or_else(|| "0.1.0".to_string());
    let batch_file_paths = tests.iter().map(|test| test.file_path.clone()).collect::<Vec<_>>();
    let dir_suffix = file_batch_dir_suffix(&batch_file_paths, &project_root);
    let runner_crate_name = runner_crate_name_for_batch_suffix(&dir_suffix);
    let rustc = match resolve_active_rustc() {
        Ok(rustc) => rustc,
        Err(error) => return failure(error.to_string()),
    };
    let rustc_target = match rustc_host_target(&rustc) {
        Ok(target) => target,
        Err(error) => return failure(error.to_string()),
    };
    let rustc_toolchain = match rustc_identity(&rustc) {
        Ok(identity) => identity,
        Err(error) => return failure(error.to_string()),
    };
    let mut build_unit_inputs = match oven_test_build_unit_inputs(&provider_plan, &requirements, &resolved) {
        Ok(inputs) => inputs,
        Err(error) => return failure(error),
    };
    if let Err(error) = crate::cli::commands::build::append_oven_interop_execution_build_inputs(
        &mut build_unit_inputs,
        manifest.as_ref(),
        &rustc_target,
    ) {
        return failure(error.message);
    }
    #[cfg(feature = "rust_inspect")]
    let rust_inspect_manifest_dir = {
        let metadata_query_paths = common::collect_rust_inspect_query_paths(&dependency_modules);
        match prepare_rust_inspect_workspace(RustInspectWorkspaceRequest {
            project_root: &project_root,
            project_name: project_name.as_str(),
            cargo_package_name: project_name.as_str(),
            rust_edition: None,
            resolved: &resolved,
            project_requirements: &requirements,
            lock_payload: None,
            cargo_lock_projection_root: None,
            clear_cargo_lock: false,
            cargo_policy_flags: Vec::new(),
            cargo_target_dir: &project_root
                .join("target/incan_tests")
                .join(&dir_suffix)
                .join("oven/rust-inspect"),
            rust_inspect_query_paths: &metadata_query_paths,
            prepare_when_empty: false,
            direct_oven_inspection: true,
            force_direct_prewarm: false,
            oven_source_authority: Some(OvenRustInspectSourceAuthorityRequest {
                project_version: &project_version,
                target: &rustc_target,
                toolchain: &rustc_toolchain,
                profile: "debug",
                features: &feature_selection.cargo_features,
                build_unit_inputs: &build_unit_inputs,
                registry_dependencies: &resolved.dependencies,
            }),
        }) {
            Ok(workspace) => workspace,
            Err(error) => return failure(error.message),
        }
    };
    let analysis = match session.analyze_modules(
        &dependency_modules,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir
            .as_ref()
            .map(|workspace| workspace.manifest_dir()),
    ) {
        Ok(analysis) => analysis,
        Err(analysis_failure) => return failure(analysis_failure.render_human()),
    };
    let Some(main_type_info) = analysis
        .type_info_for_module_path(&module_for_imports.path_segments)
        .cloned()
    else {
        return failure(format!("missing Oven test analysis for {}", first.file_path.display()));
    };
    let mut dependency_type_info = HashMap::with_capacity(source_modules.len());
    for module in &source_modules {
        let Some(type_info) = analysis.type_info_for_module_path(&module.path_segments).cloned() else {
            return failure(format!("missing Oven test analysis for {}", module.file_path.display()));
        };
        dependency_type_info.insert(module.path_segments.clone(), type_info);
    }
    let frontend_elapsed = frontend_start.elapsed();

    let generation_start = Instant::now();
    let mut codegen = IrCodegen::new();
    #[cfg(feature = "rust_inspect")]
    if let Some(workspace) = rust_inspect_manifest_dir.as_ref() {
        codegen.set_rust_inspect_manifest_dir(workspace.manifest_dir().to_path_buf());
    }
    codegen.set_provider_plan(Arc::clone(&provider_plan));
    codegen.set_stdlib_cache(analysis.stdlib_cache().clone());
    codegen.set_prechecked_type_info(main_type_info, dependency_type_info);
    codegen.set_registry_package_identity(Some(project_name.clone()));
    let compiled_sdk_modules = CompiledSdkModules::from_provider_plan(&provider_plan);
    for module in source_modules
        .iter()
        .filter(|module| compiled_sdk_modules.contains_emission_path(&module.path_segments))
    {
        codegen.add_dependency_symbol_module_with_path_segments(
            &module.name,
            &module.ast,
            module.path_segments.clone(),
        );
    }
    let emitted_source_modules = source_modules
        .iter()
        .filter(|module| !compiled_sdk_modules.contains_emission_path(&module.path_segments))
        .collect::<Vec<_>>();
    // A test runner normally compiles against provider crates, where root reachability safely prunes unused public
    // declarations. When a provider falls back to source emission, its public protocols can still construct sibling
    // public adapter models that the test root does not name directly. Retain that implementation closure whenever
    // such a source module is emitted; otherwise generated `std.io` methods can reference omitted iterator adapters.
    codegen.set_preserve_dependency_public_items(!emitted_source_modules.is_empty());
    for module in &emitted_source_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }
    if module_harnesses.is_empty() {
        codegen.set_externally_reachable_items(collect_harness_entrypoints(tests, &fixtures));
    } else {
        codegen.set_public_typecheck_module_paths(
            module_harnesses
                .iter()
                .map(|harness| harness.module_path.clone())
                .collect(),
        );
        let reachable_by_module = module_harnesses
            .iter()
            .map(|harness| {
                let file_tests = tests
                    .iter()
                    .filter(|test| test.file_path == harness.file_path)
                    .cloned()
                    .collect::<Vec<_>>();
                (
                    harness.module_path.clone(),
                    collect_harness_entrypoints(&file_tests, &harness.fixtures),
                )
            })
            .collect::<HashMap<_, _>>();
        codegen.set_externally_reachable_items_by_module(reachable_by_module);
    }
    let native_output_name = native_test_output_name(&runner_crate_name, tests);
    let generated_root = project_root.join("target/incan_tests").join(&dir_suffix);
    let mut generator = ProjectGenerator::new(&generated_root, &runner_crate_name, false);
    generator.set_provider_plan(&provider_plan);
    generator.set_sdk_path_dependencies(requirements.sdk_path_dependencies.clone());
    generator.set_package_name(Some(project_name));
    generator.set_stdlib_features(test_runner_stdlib_features_for_batch(
        &requirements.stdlib_features,
        tests,
        &fixtures,
        &module_harnesses,
    ));
    let mut runner_dependencies =
        match merge_test_runner_dependencies(&resolved.dependencies, &resolved.dev_dependencies) {
            Ok(dependencies) => dependencies,
            Err(message) => return failure(message),
        };
    runner_dependencies = match merge_test_runner_dependencies(&runner_dependencies, &requirements.dependencies) {
        Ok(dependencies) => dependencies,
        Err(message) => return failure(message),
    };
    generator.set_dependencies(runner_dependencies);
    generator.set_dev_dependencies(Vec::new());
    let generated: Result<(), String> = if emitted_source_modules.is_empty() {
        let source = match codegen.try_generate(&runner_ast) {
            Ok(source) => inject_file_test_harness(&source, tests, &project_root, &fixtures),
            Err(error) => return failure(format!("code generation error: {error}")),
        };
        generator
            .generate(&source)
            .map(|_| ())
            .map_err(|error| error.to_string())
    } else {
        let module_paths = emitted_source_modules
            .iter()
            .map(|module| module.path_segments.clone())
            .collect::<Vec<_>>();
        codegen
            .try_generate_multi_file_nested(&runner_ast, &module_paths)
            .map_err(|error| error.to_string())
            .and_then(|(mut main, mut modules)| {
                let session_cache_module = if module_harnesses.is_empty() {
                    None
                } else {
                    render_shared_session_fixture_cache_module(&module_harnesses).map(|module_code| {
                        modules.insert(vec![INCAN_SESSION_FIXTURE_MOD.to_string()], module_code);
                        INCAN_SESSION_FIXTURE_MOD
                    })
                };
                if module_harnesses.is_empty() {
                    main = inject_file_test_harness(&main, tests, &project_root, &fixtures);
                } else {
                    for harness in &module_harnesses {
                        let tests_with_indices = tests
                            .iter()
                            .enumerate()
                            .filter(|(_, test)| test.file_path == harness.file_path)
                            .collect::<Vec<_>>();
                        let file_tests = tests_with_indices
                            .iter()
                            .map(|(_, test)| (*test).clone())
                            .collect::<Vec<_>>();
                        let test_indices = tests_with_indices.iter().map(|(index, _)| *index).collect::<Vec<_>>();
                        let Some(module_code) = modules.get_mut(&harness.module_path) else {
                            return Err(format!(
                                "generated Oven test harness module `{}` was not emitted",
                                harness.module_path.join(".")
                            ));
                        };
                        *module_code = inject_file_test_harness_with_indices(
                            module_code,
                            &file_tests,
                            &test_indices,
                            &project_root,
                            &harness.fixtures,
                            session_cache_module,
                        );
                    }
                }
                generator
                    .generate_nested(&main, &modules)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
    };
    if let Err(error) = generated {
        return failure(format!("failed to generate Oven test harness: {error}"));
    }
    let generation_elapsed = generation_start.elapsed();

    let receipt_start = Instant::now();
    let mut receipt_request = OvenGeneratedProjectRequest::new(
        &project_root,
        &runner_crate_name,
        "0.1.0",
        rustc_target,
        rustc_toolchain,
        "debug",
        Vec::new(),
    )
    .with_generated_source("generated-root", generator.crate_root_path())
    .with_generated_source_tree("generated-source-tree", generator.output_dir().join("src"));
    for (name, value) in build_unit_inputs {
        receipt_request = receipt_request.with_build_unit_input(name, value);
    }
    let receipt = match receipt_generated_project(&receipt_request) {
        Ok(receipt) => receipt,
        Err(error) => return failure(error.to_string()),
    };
    let receipt_path = default_receipt_path(&generated_root);
    if let Err(error) = write_receipt(&receipt, &receipt_path) {
        return failure(error.to_string());
    }
    let receipt_elapsed = receipt_start.elapsed();

    let selection_start = Instant::now();
    let store = match commands::oven::open_default_oven_store() {
        Ok(store) => store,
        Err(error) => return failure(error.message),
    };
    let plan_selection = match crate::cli::commands::build::select_oven_direct_rustc_plan(
        &store,
        &receipt,
        &inline_path_dependencies,
    ) {
        Ok(Some(selection)) => selection,
        Ok(None) => {
            return failure(format!(
                "Oven Alpha has no compatible native test provider/dependency unit. Generated harness: {}; receipt: {}. `incan test` will not invoke Cargo; the active toolchain does not ship a compatible Oven Loaf. {}",
                generated_root.display(),
                receipt_path.display(),
                OVEN_LOAF_MISS_GUIDANCE,
            ));
        }
        Err(error) => return failure(error.message),
    };
    let selection_elapsed = selection_start.elapsed();

    // Classify caller declarations against the source projection, while retaining full-plan path authority for an
    // exact compiler-owned dependency. The complete plan may include private compiler helpers whose names overlap
    // ordinary caller dependencies such as serde_json.
    let full_artifact_plan = match &plan_selection {
        OvenTestPlanSelection::Stored(selected) => &selected.artifact_plan,
        OvenTestPlanSelection::ToolchainLoaf(native) => &native.artifact_plan,
    };
    let artifact_plan = match &plan_selection {
        OvenTestPlanSelection::Stored(selected) => {
            trusted_artifact_plan_for_source_evidence(&selected.artifact_plan, &selected.artifacts, "generated-root")
        }
        OvenTestPlanSelection::ToolchainLoaf(native) => {
            trusted_artifact_plan_for_source_evidence(&native.artifact_plan, &native.artifacts, "generated-root")
        }
    };
    let artifact_plan = match artifact_plan {
        Ok(plan) => plan,
        Err(error) => return failure(error.to_string()),
    };
    let inline_libraries_to_materialize =
        crate::cli::commands::build::declared_rust_libraries_missing_from_selected_plan(
            &inline_path_dependencies,
            &artifact_plan,
        );

    let registry_authority = match &plan_selection {
        OvenTestPlanSelection::Stored(selected) => selected
            .artifacts
            .registry_leaf_authority(&selected.artifact_root, &selected.artifact_plan),
        OvenTestPlanSelection::ToolchainLoaf(native) => native
            .artifacts
            .registry_leaf_authority(&native.artifact_root, &native.artifact_plan),
    };
    let selected_path_authority =
        crate::cli::commands::build::compiler_selected_path_authority(full_artifact_plan, Some(&provider_plan));

    if crate::cli::commands::build::has_caller_owned_project_libraries(&provider_plan) {
        let re_materialized = match &plan_selection {
            OvenTestPlanSelection::Stored(selected) => {
                match crate::cli::commands::build::rematerialize_caller_owned_libraries(
                    &provider_plan,
                    "debug",
                    &selected.artifacts,
                    &selected.artifact_root,
                    &selected.artifact_plan,
                    &rustc,
                    &generated_root,
                    registry_authority.as_ref(),
                ) {
                    Ok(libraries) => libraries,
                    Err(error) => return failure(error.message),
                }
            }
            OvenTestPlanSelection::ToolchainLoaf(native) => {
                match crate::cli::commands::build::rematerialize_caller_owned_libraries(
                    &provider_plan,
                    "debug",
                    &native.artifacts,
                    &native.artifact_root,
                    &native.artifact_plan,
                    &rustc,
                    &generated_root,
                    registry_authority.as_ref(),
                ) {
                    Ok(libraries) => libraries,
                    Err(error) => return failure(error.message),
                }
            }
        };
        if let Err(error) = crate::cli::commands::build::replace_caller_owned_package_libraries(
            &mut caller_owned_libraries,
            re_materialized,
        ) {
            return failure(error.message);
        }
    }

    let inline_libraries = match materialize_declared_rust_libraries_with_selected_path_authority(
        &generated_root.join("oven").join("inline-rust"),
        &rustc,
        &receipt.intent.target,
        "debug",
        &inline_libraries_to_materialize,
        registry_authority.as_ref(),
        selected_path_authority.as_ref(),
    ) {
        Ok(libraries) => libraries,
        Err(error) => {
            return failure(format!(
                "Oven direct-Rustc Rust dependency materialization failed: {error}"
            ));
        }
    };
    caller_owned_libraries.extend(inline_libraries);
    caller_owned_libraries.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    if caller_owned_libraries
        .windows(2)
        .any(|pair| pair[0].crate_name == pair[1].crate_name)
    {
        return failure("Oven Alpha resolved duplicate caller-owned Rust library crate names".to_string());
    }
    let bake_start = Instant::now();
    let bake = match &plan_selection {
        OvenTestPlanSelection::Stored(selected) => {
            let mut artifact_plan = match trusted_artifact_plan_for_source_evidence(
                &selected.artifact_plan,
                &selected.artifacts,
                "generated-root",
            ) {
                Ok(plan) => plan,
                Err(error) => return failure(error.to_string()),
            };
            if let Err(error) = attach_caller_owned_rustc_libraries(&mut artifact_plan, &caller_owned_libraries) {
                return failure(format!("Oven direct-rustc test compilation failed: {error}"));
            }
            bake_trusted_direct_rustc_test(&OvenTrustedDirectRustcTargetRequest {
                receipt: &receipt,
                artifacts: &selected.artifacts,
                artifact_root: &selected.artifact_root,
                artifact_plan: Some(&artifact_plan),
                rustc: &rustc,
                source: &generator.crate_root_path(),
                output: &generated_root.join("oven/debug").join(&native_output_name),
                crate_name: &runner_crate_name,
                edition: "2024",
                source_evidence_key: "generated-root",
                features: &receipt.intent.features,
                prefer_dynamic: false,
            })
        }
        OvenTestPlanSelection::ToolchainLoaf(native) => {
            let mut artifact_plan = match trusted_artifact_plan_for_source_evidence(
                &native.artifact_plan,
                &native.artifacts,
                "generated-root",
            ) {
                Ok(plan) => plan,
                Err(error) => return failure(error.to_string()),
            };
            if let Err(error) = attach_caller_owned_rustc_libraries(&mut artifact_plan, &caller_owned_libraries) {
                return failure(format!("Oven direct-rustc test compilation failed: {error}"));
            }
            bake_trusted_direct_rustc_test(&OvenTrustedDirectRustcTargetRequest {
                receipt: &receipt,
                artifacts: &native.artifacts,
                artifact_root: &native.artifact_root,
                artifact_plan: Some(&artifact_plan),
                rustc: &rustc,
                source: &generator.crate_root_path(),
                output: &generated_root.join("oven/debug").join(&native_output_name),
                crate_name: &runner_crate_name,
                edition: "2024",
                source_evidence_key: "generated-root",
                features: &receipt.intent.features,
                prefer_dynamic: false,
            })
        }
    };
    let bake = match bake {
        Ok(bake) => bake,
        Err(error) => return failure(format!("Oven direct-rustc test compilation failed: {error}")),
    };
    let bake_elapsed = bake_start.elapsed();
    let native_reused = bake.reused;
    let exact_names = tests
        .iter()
        .enumerate()
        .map(|(index, test)| {
            let harness_name = libtest_qualified_name(&harness_fn_name(test, index));
            module_harnesses
                .iter()
                .find(|harness| harness.file_path == test.file_path)
                .map(|harness| format!("{}::{harness_name}", harness.module_path.join("::")))
                .unwrap_or(harness_name)
        })
        .collect::<Vec<_>>();
    let execute_start = Instant::now();
    let report = match run_native_test_batch(&OvenNativeTestRequest {
        executable: bake.output,
        exact_names,
        environment: BTreeMap::new(),
        timeout: options.timeout,
    }) {
        Ok(report) => report,
        Err(error) => return failure(error.to_string()),
    };
    let execute_elapsed = execute_start.elapsed();
    if options.verbose && options.emit_progress {
        println!(
            "Oven test phases for {}: front-end {:.2}s, generation {:.2}s, receipt {:.2}s, selection {:.2}s, native {} {:.2}s, execution {:.2}s",
            first.file_path.display(),
            frontend_elapsed.as_secs_f64(),
            generation_elapsed.as_secs_f64(),
            receipt_elapsed.as_secs_f64(),
            selection_elapsed.as_secs_f64(),
            if native_reused { "reuse" } else { "bake" },
            bake_elapsed.as_secs_f64(),
            execute_elapsed.as_secs_f64(),
        );
    }
    if options.no_capture && !report.output.trim().is_empty() {
        print!("{}", report.output);
    }
    map_batch_results(
        tests,
        &report.output,
        start.elapsed(),
        report.success,
        &generated_root,
        &runner_crate_name,
    )
}

/// Build the portable native-closure identity used by Oven test batches.
fn oven_test_build_unit_inputs(
    provider_plan: &ProviderPlan,
    requirements: &ProjectRequirements,
    resolved: &ResolvedDependencies,
) -> Result<BTreeMap<String, String>, String> {
    let records = crate::cli::commands::build::oven_native_provider_records(
        provider_plan,
        &common::semantic_sdk_path_dependencies(requirements),
    )
    .map_err(|error| error.message)?;
    let mut dependencies = resolved.dependencies.clone();
    dependencies.extend(resolved.dev_dependencies.clone());
    let dependency_digest = digest_dependency_specs(&dependencies).map_err(|error| error.to_string())?;
    runtime_build_unit_inputs(records, &requirements.stdlib_features, dependency_digest)
}

/// Select only caller-imported Rust dependencies for the direct path-crate materializer.
///
/// Compiler-owned standard-library/provider imports are satisfied by the selected Loaf. The materializer must
/// not try to rebuild that sealed closure from the generated test project.
fn oven_test_inline_dependency_specs(
    resolved: &ResolvedDependencies,
    inline_imports: &[crate::dependency_resolver::InlineRustImport],
) -> Vec<DependencySpec> {
    let requested = inline_imports
        .iter()
        .filter(|import| import.crate_name != "incan_stdlib" && import.crate_name != "std")
        .map(|import| import.crate_name.replace('-', "_"))
        .collect::<BTreeSet<_>>();
    let mut dependencies = resolved
        .dependencies
        .iter()
        .chain(&resolved.dev_dependencies)
        .filter(|dependency| {
            let crate_name = dependency.crate_name.replace('-', "_");
            requested.contains(&crate_name)
                || dependency
                    .package
                    .as_deref()
                    .is_some_and(|package| requested.contains(&package.replace('-', "_")))
        })
        .cloned()
        .collect::<Vec<_>>();
    dependencies.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    dependencies.dedup_by(|left, right| left.crate_name == right.crate_name);
    dependencies
}

/// Extract an assertion error from stderr.
fn extract_assertion_error(stderr: &str) -> String {
    for line in stderr.lines() {
        if line.contains("assertion") || line.contains("AssertionError") {
            return line.trim().to_string();
        }
    }
    stderr.to_string()
}

/// Extract every libtest panic payload from a combined native-test transcript.
///
/// Libtest writes the location on its `panicked at` line and the actual payload on subsequent unindented lines.
/// Keeping only indented lines therefore turns an Incan assertion or fixture teardown failure into an empty report.
/// A batched harness can contain several failures, so retain every payload rather than assigning later cases the
/// first failure's location-only header.
fn extract_panic_message(output: &str) -> String {
    let mut messages = Vec::new();
    let mut payload_lines = Vec::new();
    let mut in_panic = false;

    let mut finish_panic = |payload_lines: &mut Vec<String>| {
        let payload = payload_lines.join("\n");
        if !payload.is_empty() {
            messages.push(payload);
        }
        payload_lines.clear();
    };

    for line in output.lines() {
        if line.contains("panicked at") {
            if in_panic {
                finish_panic(&mut payload_lines);
            }
            in_panic = true;
            continue;
        }
        if !in_panic {
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !payload_lines.is_empty() {
                finish_panic(&mut payload_lines);
                in_panic = false;
            }
            continue;
        }
        if trimmed.starts_with("note:") || trimmed.starts_with("---- ") || trimmed.starts_with("failures:") {
            finish_panic(&mut payload_lines);
            in_panic = false;
            continue;
        }
        payload_lines.push(trimmed.to_string());
    }
    if in_panic {
        finish_panic(&mut payload_lines);
    }

    if messages.is_empty() {
        output.to_string()
    } else {
        messages.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::sync::Arc;

    use crate::frontend::library_manifest_index::LibraryManifestIndex;
    use crate::library_manifest::LibraryManifest;
    use crate::provider::{NamespaceAuthority, ProviderIdentity, ProviderProvenance, ProviderRecord};

    #[test]
    fn oven_test_seed_compatibility_records_only_used_sdk_capabilities() -> Result<(), Box<dyn std::error::Error>> {
        let sdk = ProviderRecord {
            identity: ProviderIdentity {
                name: "incan_stdlib_testing".to_string(),
                version: "0.5.0".to_string(),
                digest: "sha256:sdk-testing".to_string(),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::Sdk {
                sdk_identity: "incan@0.5.0".to_string(),
                component_id: "stdlib-testing".to_string(),
                inventory_path: None,
            },
            authority: NamespaceAuthority::SdkReserved,
            namespace_claims: BTreeSet::from([vec!["std".to_string(), "testing".to_string()]]),
            available: true,
            enabled: true,
            manifest: Some(Arc::new(LibraryManifest::new("incan_stdlib_testing", "0.5.0"))),
            artifact: None,
            implementation_facets: Vec::new(),
        };
        let project = ProviderRecord {
            identity: ProviderIdentity {
                name: "json_provider".to_string(),
                version: "0.1.0".to_string(),
                digest: "sha256:project-json".to_string(),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::ProjectDependency {
                dependency_key: "json_provider".to_string(),
                manifest_path: PathBuf::from("json_provider.incnlib"),
            },
            authority: NamespaceAuthority::ProjectDependency {
                dependency_key: "json_provider".to_string(),
            },
            namespace_claims: BTreeSet::from([vec!["pub".to_string(), "json_provider".to_string()]]),
            available: true,
            enabled: true,
            manifest: Some(Arc::new(LibraryManifest::new("json_provider", "0.1.0"))),
            artifact: None,
            implementation_facets: Vec::new(),
        };
        let provider_plan = ProviderPlan::new(
            LibraryManifestIndex::default(),
            vec![sdk, project],
            [vec!["std".to_string(), "testing".to_string()]],
        )?;
        let inputs = oven_test_build_unit_inputs(
            &provider_plan,
            &ProjectRequirements::default(),
            &ResolvedDependencies {
                dependencies: Vec::new(),
                dev_dependencies: Vec::new(),
            },
        )?;
        let records = inputs.get("providers").ok_or("test receipt has no provider records")?;

        assert!(records.contains("incan_stdlib_testing"));
        assert!(!records.contains("json_provider"));
        Ok(())
    }

    #[test]
    fn frozen_oven_test_validation_rejects_a_missing_lock_before_scheduling() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        let tests = project.path().join("tests");
        fs::create_dir_all(&tests)?;
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"frozen_oven_test\"\nversion = \"0.1.0\"\n",
        )?;
        let test_file = tests.join("test_lock.incn");
        fs::write(&test_file, "def test_lock() -> None:\n  assert True\n")?;

        let error = match validate_oven_test_lock_policy(
            &test_file,
            &CargoPolicy::explicit(false, false, true, Vec::new()),
            &FeatureSelection::default(),
            None,
        ) {
            Ok(()) => return Err("a frozen Oven test created an incan.lock through Cargo".into()),
            Err(error) => error,
        };

        assert!(error.message.contains("incan.lock is missing; run `incan lock`"));
        Ok(())
    }

    fn parsed_module_for_import_context(
        name: &str,
        path: &str,
        source: &str,
    ) -> Result<ParsedModule, Box<dyn std::error::Error>> {
        let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
        Ok(ParsedModule {
            name: name.to_string(),
            path_segments: vec![name.to_string()],
            file_path: PathBuf::from(path),
            source: source.to_string(),
            ast,
        })
    }

    #[test]
    fn test_dependency_inline_imports_keep_source_imports_normal() -> Result<(), Box<dyn std::error::Error>> {
        let test_module = parsed_module_for_import_context(
            "test",
            "tests/test_dataset.incn",
            "from rust::tokio @ \"1\" import spawn\n",
        )?;
        let source_module = parsed_module_for_import_context(
            "dataset",
            "src/dataset.incn",
            "from rust::datafusion @ \"53\" import SessionContext\n",
        )?;

        let imports = collect_test_dependency_inline_imports(&test_module, &[source_module]);
        let tokio = imports
            .iter()
            .find(|import| import.crate_name == "tokio")
            .ok_or("expected tokio import")?;
        let datafusion = imports
            .iter()
            .find(|import| import.crate_name == "datafusion")
            .ok_or("expected datafusion import")?;

        assert!(tokio.is_test_context);
        assert!(!datafusion.is_test_context);
        Ok(())
    }

    #[test]
    fn merge_test_runner_dependencies_promotes_dev_deps_into_dependencies() {
        use crate::manifest::{DependencySource, DependencySpec};

        let deps = vec![DependencySpec {
            crate_name: "serde".to_string(),
            version: Some("1.0".to_string()),
            features: vec![],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }];
        let dev_deps = vec![DependencySpec {
            crate_name: "tokio".to_string(),
            version: Some("1".to_string()),
            features: vec!["macros".to_string(), "rt-multi-thread".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }];

        let merged = match merge_test_runner_dependencies(&deps, &dev_deps) {
            Ok(merged) => merged,
            Err(err) => panic!("expected merge to succeed: {err}"),
        };
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|dep| dep.crate_name == "serde"));
        assert!(merged.iter().any(|dep| dep.crate_name == "tokio"));
    }

    #[test]
    fn merge_test_runner_dependencies_rejects_conflicting_duplicates() {
        use crate::manifest::{DependencySource, DependencySpec};

        let deps = vec![DependencySpec {
            crate_name: "tokio".to_string(),
            version: Some("1".to_string()),
            features: vec!["time".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }];
        let dev_deps = vec![DependencySpec {
            crate_name: "tokio".to_string(),
            version: Some("1".to_string()),
            features: vec!["macros".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }];

        let error = match merge_test_runner_dependencies(&deps, &dev_deps) {
            Ok(merged) => panic!("expected conflict, got merged dependencies: {merged:?}"),
            Err(err) => err,
        };
        assert!(error.contains("tokio"));
        assert!(error.contains("conflicts"));
    }

    #[test]
    fn parse_libtest_outcomes_detects_ok_and_failed() {
        let out = r#"
test __incan_file_tests::incan_harness_0_a ... ok
test __incan_file_tests::incan_harness_1_b ... FAILED
test result: FAILED. 1 passed; 1 failed
"#;
        let m = parse_libtest_outcomes(out);
        assert_eq!(m.get("__incan_file_tests::incan_harness_0_a"), Some(&true));
        assert_eq!(m.get("__incan_file_tests::incan_harness_1_b"), Some(&false));
    }

    #[test]
    fn parse_libtest_outcomes_normalizes_prefixed_names() {
        let out = r#"
test test_runner_76001490ba86f677::__incan_file_tests::incan_harness_0_a ... ok
test test_runner_76001490ba86f677::__incan_file_tests::incan_harness_1_b ... FAILED
"#;
        let m = parse_libtest_outcomes(out);
        assert_eq!(m.get("__incan_file_tests::incan_harness_0_a"), Some(&true));
        assert_eq!(m.get("__incan_file_tests::incan_harness_1_b"), Some(&false));
    }

    #[test]
    fn successful_native_batch_preserves_a_passing_harness_result_issue996() {
        let test = TestInfo {
            file_path: PathBuf::from("tests/test_smoke.incn"),
            function_name: "test_smoke_reports_pass".to_string(),
            is_async: false,
            markers: Vec::new(),
            required_fixtures: Vec::new(),
            parameter_names: Vec::new(),
            timeout: None,
            parametrize_call: None,
        };
        let results = map_batch_results(
            std::slice::from_ref(&test),
            "running 1 test\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n",
            Duration::from_millis(5),
            true,
            Path::new("target/incan/test-runner"),
            "test_runner",
        );

        assert!(matches!(
            results.first().map(|(_, result)| result),
            Some(TestResult::Passed(_))
        ));
    }

    #[test]
    fn extracts_unindented_libtest_panic_payloads_from_a_batch() {
        let transcript = r#"
thread 'test_runner::__incan_file_tests::incan_harness_0_message' panicked at src/lib.rs:17:13:
AssertionError: custom boom

thread 'test_runner::__incan_file_tests::incan_harness_1_teardown' panicked at src/lib.rs:41:9:
fixture teardown failed:
child teardown failed
parent teardown failed
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
"#;

        let detail = extract_panic_message(transcript);

        assert!(detail.contains("AssertionError: custom boom"));
        assert!(detail.contains("fixture teardown failed:"));
        assert!(detail.contains("child teardown failed"));
        assert!(detail.contains("parent teardown failed"));
        assert!(!detail.contains("panicked at"));
    }

    #[test]
    fn runner_crate_name_is_derived_from_batch_suffix() {
        let name = runner_crate_name_for_batch_suffix("batch_76001490ba86f677");
        assert_eq!(name, "test_runner_76001490ba86f677");
    }

    #[test]
    fn native_test_outputs_do_not_alias_distinct_execution_groups() {
        let normal = TestInfo {
            file_path: PathBuf::from("tests/test_groups.incn"),
            function_name: "test_normal".to_string(),
            is_async: false,
            markers: vec![],
            required_fixtures: vec![],
            parameter_names: vec![],
            timeout: None,
            parametrize_call: None,
        };
        let expected_failure = TestInfo {
            function_name: "test_expected_failure".to_string(),
            ..normal.clone()
        };
        let runner = "test_runner_groups";

        assert_ne!(
            native_test_output_name(runner, std::slice::from_ref(&normal)),
            native_test_output_name(runner, std::slice::from_ref(&expected_failure)),
        );
        assert_eq!(
            native_test_output_name(runner, &[normal.clone(), expected_failure.clone()]),
            native_test_output_name(runner, &[normal, expected_failure]),
        );
    }

    #[test]
    fn batch_suffix_is_path_independent_but_content_sensitive() -> Result<(), Box<dyn std::error::Error>> {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        let first_test = first.path().join("tests/test_cache.incn");
        let second_test = second.path().join("tests/test_cache.incn");
        fs::create_dir_all(first_test.parent().ok_or("missing first test parent")?)?;
        fs::create_dir_all(second_test.parent().ok_or("missing second test parent")?)?;
        fs::write(&first_test, "def test_cache() -> None:\n  assert True\n")?;
        fs::write(&second_test, "def test_cache() -> None:\n  assert True\n")?;

        let first_suffix = file_batch_dir_suffix(std::slice::from_ref(&first_test), first.path());
        let second_suffix = file_batch_dir_suffix(std::slice::from_ref(&second_test), second.path());
        assert_eq!(first_suffix, second_suffix);

        fs::write(&second_test, "def test_cache() -> None:\n  assert False\n")?;
        let changed_suffix = file_batch_dir_suffix(&[second_test], second.path());
        assert_ne!(first_suffix, changed_suffix);
        Ok(())
    }

    #[test]
    fn nested_pub_import_dedupe_preserves_exact_child_identity_issue948() -> Result<(), String> {
        let source = "from pub::modulelib.hyperquant.index import build\n\
                      from pub::modulelib.hyperquant.search import build\n";
        let tokens = crate::frontend::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let mut program = crate::frontend::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;

        dedupe_import_declarations(&mut program);

        let imports = program
            .declarations
            .iter()
            .filter(|declaration| matches!(declaration.node, Declaration::Import(_)))
            .count();
        assert_eq!(imports, 2);
        Ok(())
    }

    #[test]
    fn module_name_for_segments_disambiguates_join_collisions() {
        let flat = module_name_for_segments(&["a_b".to_string()]);
        let nested = module_name_for_segments(&["a".to_string(), "b".to_string()]);

        assert_ne!(flat, nested);
        assert!(flat.starts_with("a_b_"));
        assert!(nested.starts_with("a_b_"));
    }

    #[test]
    fn manifestless_test_below_target_directory_owns_its_parent_root() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let test_file = workspace.path().join("target/ci-nonroot/tmp/fixture/test_a.incn");
        let parent = test_file.parent().ok_or("test fixture should have a parent")?;
        fs::create_dir_all(parent)?;
        fs::write(&test_file, "def test_a() -> None:\n    pass\n")?;
        let expected_root = fs::canonicalize(parent)?;

        assert_eq!(
            infer_project_root_without_manifest(&test_file),
            expected_root,
            "a manifest-less fixture below a caller target directory must not inherit that caller as its project root"
        );
        Ok(())
    }

    #[test]
    fn inject_file_test_harness_emits_tests_module() {
        let rust = "fn test_a() {}\nfn test_b() {}\n";
        let tests = vec![
            TestInfo {
                file_path: PathBuf::from("t.incn"),
                function_name: "test_a".to_string(),
                is_async: false,
                markers: vec![],
                required_fixtures: vec![],
                parameter_names: vec![],
                timeout: None,
                parametrize_call: None,
            },
            TestInfo {
                file_path: PathBuf::from("t.incn"),
                function_name: "test_b".to_string(),
                is_async: false,
                markers: vec![],
                required_fixtures: vec![],
                parameter_names: vec![],
                timeout: None,
                parametrize_call: None,
            },
        ];
        let g = inject_file_test_harness(rust, &tests, Path::new("."), &HashMap::new());
        assert!(g.contains("mod __incan_file_tests"));
        assert!(g.contains("fn incan_harness_0_test_a"));
        assert!(g.contains("fn incan_harness_1_test_b"));
        assert!(g.contains("set_current_dir"));
        assert!(g.contains("super::test_a();"));
        assert!(g.contains("super::test_b();"));
    }

    #[test]
    fn inject_file_test_harness_wraps_async_tests_and_fixtures() {
        let rust = "async fn resource() -> i64 { 42 }\nasync fn test_async(resource: i64) {}\n";
        let tests = vec![TestInfo {
            file_path: PathBuf::from("t.incn"),
            function_name: "test_async".to_string(),
            is_async: true,
            markers: vec![],
            required_fixtures: vec!["resource".to_string()],
            parameter_names: vec!["resource".to_string()],
            timeout: None,
            parametrize_call: None,
        }];
        let mut fixtures = HashMap::new();
        fixtures.insert(
            "resource".to_string(),
            FixtureExecutionInfo {
                params: Vec::new(),
                scope: FixtureScope::Function,
                has_teardown: true,
                is_async: true,
                return_rust_type: Some("i64".to_string()),
                state_rust_type: Some("i64".to_string()),
                teardown: Some(YieldFixtureTeardown {
                    teardown_function: "__incan_fixture_teardown_resource".to_string(),
                    captures: Vec::new(),
                    value_ty: Type::Simple("int".to_string()),
                }),
            },
        );

        let generated = inject_file_test_harness(rust, &tests, Path::new("."), &fixtures);

        assert!(generated.contains("__INCAN_ASYNC_RUNTIME"));
        assert!(generated.contains("__incan_async_block_on(super::resource())"));
        assert!(generated.contains("__incan_async_block_on(super::test_async(__incan_fixture_value_0_resource))"));
        assert!(generated.contains("__incan_run_teardown"));
        assert!(generated.contains("__incan_async_block_on(super::__incan_fixture_teardown_resource())"));
    }
}
