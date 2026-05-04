use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::backend::{IrCodegen, ProjectGenerator};
use crate::cli::commands;
use crate::cli::commands::common::{self, CargoPolicy, ProjectRequirements};
#[cfg(feature = "rust_inspect")]
use crate::cli::commands::common::{
    collect_rust_inspect_query_paths, ensure_rust_inspect_workspace, prewarm_rust_inspect_workspace,
};
use crate::cli::prelude::ParsedModule;
use crate::dependency_resolver::ResolvedDependencies;
use crate::dependency_resolver::resolve_dependencies;
use crate::frontend::ast::{
    AssertKind, AssertStmt, CallArg, Declaration, DictEntry, Expr, ImportItem, ImportKind, ListEntry, ParamKind,
    Program, Spanned, Statement, Type,
};
use crate::frontend::decorator_resolution;
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::testing_markers::{TestingMarkerKind, load_testing_marker_semantics, resolve_testing_marker_kind};
use crate::frontend::vocab_desugar_pass;
use crate::frontend::{lexer, parser};
use crate::lockfile::CargoFeatureSelection;
use crate::manifest::ProjectManifest;
use sha2::{Digest, Sha256};

use super::module_graph::collect_source_modules_for_test;
use super::types::{FixtureScope, TestInfo, TestResult};

/// Generated `#[cfg(test)]` module that wraps Incan test functions as Rust `#[test]` cases, one `cargo test` per file.
const INCAN_FILE_TEST_MOD: &str = "__incan_file_tests";

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TestExecutionOptions {
    pub no_capture: bool,
    pub timeout: Option<Duration>,
    pub jobs: usize,
    pub verbose: bool,
    pub emit_progress: bool,
}

const TEST_HARNESS_PREHEAT_FINGERPRINT_FILE: &str = ".incan_preheat_fingerprint";
const TEST_HARNESS_PREHEAT_LOCK_FILE: &str = ".incan_preheat.lock";
const TEST_HARNESS_PREHEAT_STALE_LOCK_SECS: u64 = 30 * 60;

fn parse_isolated_target_env(raw: Option<&str>) -> bool {
    matches!(raw.map(str::trim), Some("1" | "true" | "yes" | "on"))
}

/// Return whether generated test-harness preheat should run for the supplied environment value.
fn parse_test_preheat_env(raw: Option<&str>) -> bool {
    !matches!(raw.map(str::trim), Some("0" | "false" | "no" | "off"))
}

/// Return whether generated test-harness preheat is enabled for this process.
fn test_preheat_enabled() -> bool {
    parse_test_preheat_env(std::env::var("INCAN_TEST_PREHEAT").ok().as_deref())
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
        rust_module_path: ast.rust_module_path.clone(),
        warnings: ast.warnings.clone(),
    }
}

/// Return whether a top-level function is a `std.testing.fixture` declaration.
fn has_fixture_decorator(
    decorators: &[crate::frontend::ast::Spanned<crate::frontend::ast::Decorator>],
    aliases: &HashMap<String, Vec<String>>,
) -> bool {
    let Ok(semantics) = load_testing_marker_semantics() else {
        return false;
    };
    decorators.iter().any(|decorator| {
        resolve_testing_marker_kind(&decorator.node, aliases, &semantics) == Some(TestingMarkerKind::Fixture)
    })
}

/// Remove shadowed fixture functions so execution uses the same "nearest fixture wins" rule as collection.
fn prune_shadowed_fixture_declarations(ast: &mut Program) {
    let aliases = decorator_resolution::collect_import_aliases(ast);
    let mut last_fixture_decl = HashMap::new();
    for (index, decl) in ast.declarations.iter().enumerate() {
        if let Declaration::Function(func) = &decl.node
            && has_fixture_decorator(&func.decorators, &aliases)
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
                && has_fixture_decorator(&func.decorators, &aliases)
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
                ImportKind::PubFrom { library, items } => {
                    let prefix = format!("pub-from:{:?}:{library}", import.visibility);
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

#[derive(Debug, Default)]
struct TopLevelNames {
    types: HashSet<String>,
    values: HashSet<String>,
}

/// Collect top-level Rust item names that would collide if multiple Incan files were concatenated.
fn collect_top_level_decl_names(program: &Program) -> TopLevelNames {
    /// Add the Rust type/value namespace names contributed by one declaration.
    fn collect_from_decl(decl: &Declaration, names: &mut TopLevelNames) {
        match decl {
            Declaration::Const(decl) => {
                names.values.insert(decl.name.clone());
            }
            Declaration::Static(decl) => {
                names.values.insert(decl.name.clone());
            }
            Declaration::Model(decl) => {
                names.types.insert(decl.name.clone());
                names.values.insert(decl.name.clone());
            }
            Declaration::Class(decl) => {
                names.types.insert(decl.name.clone());
                names.values.insert(decl.name.clone());
            }
            Declaration::Trait(decl) => {
                names.types.insert(decl.name.clone());
            }
            Declaration::Alias(decl) => {
                names.values.insert(decl.name.clone());
            }
            Declaration::TypeAlias(decl) => {
                names.types.insert(decl.name.clone());
            }
            Declaration::Newtype(decl) => {
                names.types.insert(decl.name.clone());
                names.values.insert(decl.name.clone());
            }
            Declaration::Enum(decl) => {
                names.types.insert(decl.name.clone());
            }
            Declaration::Function(decl) => {
                names.values.insert(decl.name.clone());
            }
            Declaration::TestModule(decl) => {
                for nested in &decl.body {
                    collect_from_decl(&nested.node, names);
                }
            }
            Declaration::Import(_) | Declaration::Docstring(_) => {}
        }
    }

    let mut names = TopLevelNames::default();
    for decl in &program.declarations {
        collect_from_decl(&decl.node, &mut names);
    }
    names
}

/// Return whether concatenating source files into one worker harness would collide at Rust module scope.
///
/// Worker batches can share one process only when their source files can coexist in the generated crate. If two files
/// define the same model, function, or other top-level Rust item, the runner falls back to per-file harnesses.
fn batch_has_cross_file_top_level_collision(
    sources_by_file: &[(PathBuf, String)],
    library_imported_vocab: Option<&parser::ImportedLibraryVocab>,
) -> bool {
    if sources_by_file.len() <= 1 {
        return false;
    }

    let mut type_owner: HashMap<String, PathBuf> = HashMap::new();
    let mut value_owner: HashMap<String, PathBuf> = HashMap::new();
    for (path, source) in sources_by_file {
        let Ok(tokens) = lexer::lex(source) else {
            return false;
        };
        let Ok(ast) =
            parser::parse_with_context(&tokens, Some(path.to_string_lossy().as_ref()), library_imported_vocab)
        else {
            return false;
        };
        let names = collect_top_level_decl_names(&ast_with_inline_test_declarations(&ast));
        for name in names.types {
            if type_owner
                .insert(name, path.clone())
                .is_some_and(|owner| owner != *path)
            {
                return true;
            }
        }
        for name in names.values {
            if value_owner
                .insert(name, path.clone())
                .is_some_and(|owner| owner != *path)
            {
                return true;
            }
        }
    }

    false
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

/// Shared Cargo `target/` directory for generated test crates in a package.
///
/// By default this reuses the project's main `target/` so existing dependency artifacts are shared across regular
/// builds and `incan test` runs for better DX.
///
/// Set `INCAN_TEST_ISOLATED_TARGET_DIR` to one of `1|true|yes|on` to use `target/incan_test_runner` instead.
fn shared_cargo_target_dir(project_root: &Path) -> PathBuf {
    let absolute_project_root = if project_root.is_absolute() {
        project_root.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(project_root)
    } else {
        project_root.to_path_buf()
    };

    if parse_isolated_target_env(std::env::var("INCAN_TEST_ISOLATED_TARGET_DIR").ok().as_deref()) {
        absolute_project_root.join("target").join("incan_test_runner")
    } else {
        absolute_project_root.join("target")
    }
}

/// Shared front-end + dependency work for one test file, reused across parametrized variants and multiple tests in the
/// same `.incn` file within a single `incan test` session.
pub(super) struct PreparedTestFile {
    pub library_manifest_index: LibraryManifestIndex,
    pub ast: Program,
    pub fixture_teardowns: HashMap<String, YieldFixtureTeardown>,
    pub source_modules: Vec<ParsedModule>,
    pub project_root: PathBuf,
    pub resolved: ResolvedDependencies,
    pub project_requirements: ProjectRequirements,
    pub lock_payload: Option<String>,
    #[cfg(feature = "rust_inspect")]
    pub rust_inspect_manifest_dir: PathBuf,
}

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
            if let crate::frontend::ast::FStringPart::Expr(expr) = part {
                expr_references_name(&expr.node, name)
            } else {
                false
            }
        }),
        Expr::Range { start, end, .. } => {
            expr_references_name(&start.node, name) || expr_references_name(&end.node, name)
        }
        Expr::Literal(_) | Expr::SelfExpr | Expr::Yield(None) | Expr::Surface(_) => false,
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
fn split_yield_fixture_declarations(ast: &mut Program) -> Result<HashMap<String, YieldFixtureTeardown>, String> {
    let aliases = decorator_resolution::collect_import_aliases(ast);
    let mut teardowns = HashMap::new();
    let mut additional = Vec::new();

    for decl in &mut ast.declarations {
        let Declaration::Function(func) = &mut decl.node else {
            continue;
        };
        if !has_fixture_decorator(&func.decorators, &aliases) {
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
/// If no conventional anchor is present, fall back to the caller cwd when the test
/// file lives underneath it; otherwise use the test file's parent directory.
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

    if let Ok(cwd) = std::env::current_dir() {
        let cwd = fs::canonicalize(&cwd).unwrap_or(cwd);
        if absolute_test_path.starts_with(&cwd) {
            return cwd;
        }
    }

    absolute_test_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

/// Compute the session-local cache key for dependency, lockfile, and rust-inspect prep.
fn compute_test_prep_cache_key(
    test_path: &Path,
    source: &str,
    source_modules: &[ParsedModule],
    manifest: Option<&ProjectManifest>,
    cargo: &CargoFeatureSelection,
    cargo_policy: &CargoPolicy,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"incan_test_prep/1\0");
    hasher.update(canonical_path_for_cache_key(test_path).to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(source.as_bytes());
    hasher.update(b"\0");

    let mut sorted_mods: Vec<&ParsedModule> = source_modules.iter().collect();
    sorted_mods.sort_by(|a, b| a.file_path.cmp(&b.file_path));
    for m in sorted_mods {
        hasher.update(canonical_path_for_cache_key(&m.file_path).to_string_lossy().as_bytes());
        hasher.update(b"\0");
        hasher.update(m.source.as_bytes());
        hasher.update(b"\0|\0");
    }

    match manifest {
        Some(m) => {
            hasher.update(b"manifest\0");
            hasher.update(m.path().to_string_lossy().as_bytes());
            hasher.update(b"\0");
            // Distinguish read errors from an empty manifest file (both would otherwise contribute no bytes).
            match fs::read_to_string(m.path()) {
                Ok(body) => {
                    hasher.update(b"ok\0");
                    hasher.update(body.as_bytes());
                }
                Err(_) => hasher.update(b"err\0"),
            }
            hasher.update(b"\0");
        }
        None => hasher.update(b"nomanifest\0"),
    }

    for f in &cargo.cargo_features {
        hasher.update(f.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update([cargo.cargo_no_default_features as u8]);
    hasher.update([cargo.cargo_all_features as u8]);
    hasher.update([cargo_policy.offline as u8]);
    hasher.update([cargo_policy.locked as u8]);
    hasher.update([cargo_policy.frozen as u8]);
    for arg in &cargo_policy.extra_args {
        hasher.update(arg.as_bytes());
        hasher.update(b"\0");
    }

    format!("v1:{}", hex::encode(hasher.finalize()))
}

/// Merge stdlib feature flags from previously prepared files with the current file requirements.
///
/// The rust-inspect workspace lives under one shared `target/incan_lock` directory per package. If files in a single
/// `incan test` session require different stdlib features, a non-monotonic feature set can cause workspace
/// fingerprint churn and expensive mid-run rewrites. Keeping a session-local feature union avoids that churn.
fn merge_rust_inspect_stdlib_features<'a>(
    existing_feature_sets: impl Iterator<Item = &'a [String]>,
    current_features: &[String],
) -> Vec<String> {
    let mut merged: BTreeSet<String> = current_features.iter().cloned().collect();
    for features in existing_feature_sets {
        merged.extend(features.iter().cloned());
    }
    merged.into_iter().collect()
}

/// Promote project dev dependencies into ordinary dependencies for generated test-runner crates.
///
/// `incan test` generates a library crate and runs `cargo test` against that crate. Because the generated user/test
/// code lives under `src/`, anything it imports must be available as a normal dependency, not only under
/// `[dev-dependencies]`.
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
fn file_batch_dir_suffix(file_paths: &[PathBuf]) -> String {
    let mut hasher = Sha256::new();
    let mut paths = file_paths.to_vec();
    paths.sort();
    paths.dedup();
    let multi_file = paths.len() > 1;
    for file_path in paths {
        let p = fs::canonicalize(&file_path).unwrap_or(file_path);
        hasher.update(p.to_string_lossy().as_bytes());
        if multi_file {
            hasher.update(b"\0");
        }
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
struct FixtureExecutionInfo {
    params: Vec<String>,
    scope: FixtureScope,
    has_teardown: bool,
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

/// Return the local generated Rust binding that stores one fixture's setup/teardown state.
fn fixture_state_ident(index: usize, name: &str) -> String {
    format!("__incan_fixture_state_{index}_{}", safe_fixture_ident(name))
}

/// Return the local generated Rust binding that is passed to the user test as the fixture value.
fn fixture_value_ident(index: usize, name: &str) -> String {
    format!("__incan_fixture_value_{index}_{}", safe_fixture_ident(name))
}

/// Generate an expression that calls a fixture, recursively filling fixture dependencies.
fn fixture_arg(
    name: &str,
    index: usize,
    setup: &mut String,
    fixtures: &HashMap<String, FixtureExecutionInfo>,
    created_builtins: &mut HashSet<String>,
    teardown_steps: &mut Vec<String>,
    visiting: &mut Vec<String>,
) -> String {
    if let Some(expr) = builtin_fixture_arg(name, index, setup, created_builtins) {
        return expr;
    }

    if visiting.iter().any(|existing| existing == name) {
        return format!("super::{name}()");
    }
    visiting.push(name.to_string());
    let args = fixtures
        .get(name)
        .map(|fixture| {
            fixture
                .params
                .iter()
                .map(|param| {
                    fixture_arg(
                        param,
                        index,
                        setup,
                        fixtures,
                        created_builtins,
                        teardown_steps,
                        visiting,
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let _ = visiting.pop();
    let Some(fixture) = fixtures.get(name) else {
        return format!("super::{name}({args})");
    };
    let Some(return_rust_type) = fixture.return_rust_type.as_ref() else {
        return format!("super::{name}({args})");
    };
    if fixture.has_teardown {
        let Some(teardown) = &fixture.teardown else {
            return format!("super::{name}({args})");
        };
        if fixture.scope == FixtureScope::Function {
            let state_ident = fixture_state_ident(index, name);
            let value_ident = fixture_value_ident(index, name);
            setup.push_str(&format!("        let {state_ident} = super::{name}({args});\n"));
            if teardown.captures.is_empty() {
                setup.push_str(&format!("        let {value_ident} = {state_ident};\n"));
                teardown_steps.push(format!("super::{}();", teardown.teardown_function));
            } else {
                let capture_names = teardown
                    .captures
                    .iter()
                    .map(|capture| format!("__incan_fixture_capture_{}_{}", safe_fixture_ident(name), capture.name))
                    .collect::<Vec<_>>();
                setup.push_str(&format!(
                    "        let ({value_ident}, {}) = {state_ident};\n",
                    capture_names.join(", ")
                ));
                teardown_steps.push(format!(
                    "super::{}({});",
                    teardown.teardown_function,
                    capture_names.join(", ")
                ));
            }
            return value_ident;
        }
        let static_name = fixture_cache_static_name(name);
        if teardown.captures.is_empty() {
            return format!(
                "{{\n\
                     let __incan_cache = {static_name}.get_or_init(|| std::sync::Mutex::new(None));\n\
                     let Ok(mut __incan_guard) = __incan_cache.lock() else {{ panic!(\"fixture cache `{name}` is poisoned\"); }};\n\
                     if __incan_guard.is_none() {{ *__incan_guard = Some(super::{name}({args})); }}\n\
                     let Some(__incan_value) = __incan_guard.as_ref() else {{ panic!(\"fixture cache `{name}` was not initialized\"); }};\n\
                     __incan_value.clone()\n\
                 }}"
            );
        }
        return format!(
            "{{\n\
                     let __incan_cache = {static_name}.get_or_init(|| std::sync::Mutex::new(None));\n\
                     let Ok(mut __incan_guard) = __incan_cache.lock() else {{ panic!(\"fixture cache `{name}` is poisoned\"); }};\n\
                     if __incan_guard.is_none() {{ *__incan_guard = Some(super::{name}({args})); }}\n\
                     let Some(__incan_state) = __incan_guard.as_ref() else {{ panic!(\"fixture cache `{name}` was not initialized\"); }};\n\
                     let __incan_value: &{return_rust_type} = &__incan_state.0;\n\
                     __incan_value.clone()\n\
                 }}"
        );
    }
    if fixture.scope == FixtureScope::Function {
        return format!("super::{name}({args})");
    }

    let static_name = fixture_cache_static_name(name);
    format!(
        "{{\n\
                 let __incan_cache = {static_name}.get_or_init(|| std::sync::Mutex::new(None));\n\
                 let Ok(mut __incan_guard) = __incan_cache.lock() else {{ panic!(\"fixture cache `{name}` is poisoned\"); }};\n\
                 if __incan_guard.is_none() {{ *__incan_guard = Some(Box::new(super::{name}({args}))); }}\n\
                 let Some(__incan_boxed) = __incan_guard.as_ref() else {{ panic!(\"fixture cache `{name}` was not initialized\"); }};\n\
                 let Some(__incan_value) = __incan_boxed.downcast_ref::<{return_rust_type}>() else {{ panic!(\"fixture cache `{name}` had an unexpected type\"); }};\n\
                 __incan_value.clone()\n\
             }}"
    )
}

/// Generate the body statement that invokes one collected test case.
fn harness_call(test: &TestInfo, index: usize, fixtures: &HashMap<String, FixtureExecutionInfo>) -> String {
    let mut setup = String::new();
    let mut args = Vec::new();
    let mut teardown_steps = Vec::new();
    let mut used_fixtures = HashSet::new();
    let mut created_builtins = HashSet::new();
    let parametrize = test.parametrize_call.as_ref();

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
            args.push(fixture_arg(
                param_name,
                index,
                &mut setup,
                fixtures,
                &mut created_builtins,
                &mut teardown_steps,
                &mut Vec::new(),
            ));
        }
    }

    if test.parameter_names.is_empty() {
        if let Some(call) = parametrize {
            args.extend(call.rust_arguments.clone());
        }
        for fixture in &test.required_fixtures {
            used_fixtures.insert(fixture.clone());
            args.push(fixture_arg(
                fixture,
                index,
                &mut setup,
                fixtures,
                &mut created_builtins,
                &mut teardown_steps,
                &mut Vec::new(),
            ));
        }
    }

    for fixture in &test.required_fixtures {
        if !used_fixtures.contains(fixture) {
            let expr = fixture_arg(
                fixture,
                index,
                &mut setup,
                fixtures,
                &mut created_builtins,
                &mut teardown_steps,
                &mut Vec::new(),
            );
            setup.push_str(&format!("        let _ = {expr};\n"));
        }
    }

    let joined = args.join(", ");
    if teardown_steps.is_empty() {
        return format!("{setup}        super::{}({joined});\n", test.function_name);
    }

    let mut teardown = String::new();
    for step in teardown_steps.iter().rev() {
        teardown.push_str("        ");
        teardown.push_str(step);
        teardown.push('\n');
    }
    format!(
        "{setup}        let __incan_test_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {{\n\
                     super::{}({joined});\n\
                 }}));\n\
         {teardown}        if let Err(__incan_panic) = __incan_test_result {{ std::panic::resume_unwind(__incan_panic); }}\n",
        test.function_name
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
) -> HashMap<String, FixtureExecutionInfo> {
    let aliases = decorator_resolution::collect_import_aliases(ast);
    let semantics = load_testing_marker_semantics().ok();
    ast.declarations
        .iter()
        .filter_map(|decl| {
            if let crate::frontend::ast::Declaration::Function(func) = &decl.node {
                let is_fixture = semantics.as_ref().is_some_and(|semantics| {
                    func.decorators.iter().any(|decorator| {
                        resolve_testing_marker_kind(&decorator.node, &aliases, semantics)
                            == Some(TestingMarkerKind::Fixture)
                    })
                });
                if !is_fixture {
                    return None;
                }
                let mut scope = FixtureScope::Function;
                if let Some(semantics) = semantics.as_ref() {
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
                }
                let teardown = fixture_teardowns.get(&func.name).cloned();
                Some((
                    func.name.clone(),
                    FixtureExecutionInfo {
                        params: func.params.iter().map(|param| param.node.name.clone()).collect(),
                        scope,
                        has_teardown: teardown.is_some(),
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

/// Append a `#[cfg(test)]` module with one `#[test]` per collected case so `cargo test` runs an entire file in one
/// shot.
///
/// The generated harness resets the process cwd to the source project root before each test so fixture paths behave
/// the same way as ordinary `incan run/build/test` entrypoints rather than inheriting the generated temp crate path.
fn inject_file_test_harness(
    rust_code: &str,
    tests: &[TestInfo],
    project_root: &Path,
    fixtures: &HashMap<String, FixtureExecutionInfo>,
) -> String {
    let mut out = rust_code.to_string();
    let project_root_literal = project_root.to_string_lossy().to_string();
    out.push_str("\n\n#[cfg(test)]\nmod ");
    out.push_str(INCAN_FILE_TEST_MOD);
    out.push_str(" {\n");
    for (name, fixture) in fixtures {
        if fixture.scope == FixtureScope::Function {
            continue;
        }
        if fixture.has_teardown {
            if let Some(state_rust_type) = &fixture.state_rust_type {
                out.push_str("static ");
                out.push_str(&fixture_cache_static_name(name));
                out.push_str(": std::sync::OnceLock<std::sync::Mutex<Option<");
                out.push_str(state_rust_type);
                out.push_str(">>> = std::sync::OnceLock::new();\n");
            }
        } else if fixture.return_rust_type.is_some() {
            out.push_str("static ");
            out.push_str(&fixture_cache_static_name(name));
            out.push_str(
                ": std::sync::OnceLock<std::sync::Mutex<Option<Box<dyn std::any::Any + Send>>>> = std::sync::OnceLock::new();\n",
            );
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
    let teardown_fixtures = ordered_teardown_fixtures(tests, fixtures);
    for (index, t) in tests.iter().enumerate() {
        let fname = harness_fn_name(t, index);
        let call = harness_call(t, index, fixtures);
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
            let static_name = fixture_cache_static_name(name);
            out.push_str(&format!(
                "        if let Some(__incan_cache) = {static_name}.get() {{\n\
                         let Ok(mut __incan_guard) = __incan_cache.lock() else {{ panic!(\"fixture cache `{name}` is poisoned\"); }};\n\
                         if let Some(__incan_state) = __incan_guard.take() {{\n"
            ));
            if teardown.captures.is_empty() {
                out.push_str("            let _ = __incan_state;\n");
                out.push_str(&format!("            super::{}();\n", teardown.teardown_function));
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
                    "            super::{}({});\n",
                    teardown.teardown_function,
                    capture_names.join(", ")
                ));
            }
            out.push_str("                         }\n        }\n");
        }
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

/// Parse `cargo test` / libtest lines: `test <name> ... ok|FAILED`.
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

/// Best-effort extraction of failure output for one harness `fn_name` from combined `cargo test` stdout/stderr.
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

/// Turn one batched `cargo test` run into per-[`TestInfo`] results.
///
/// On compile failure, every test shares `compile_message`. Otherwise outcomes come from [`parse_libtest_outcomes`];
/// wall time is split evenly across tests for display.
fn map_batch_results(
    tests: &[TestInfo],
    combined_output: &str,
    elapsed: std::time::Duration,
    compile_failed: bool,
    compile_message: &str,
    manifest_path: &Path,
    crate_name: &str,
) -> Vec<(TestInfo, TestResult)> {
    if compile_failed {
        let msg = compile_message.to_string();
        return tests
            .iter()
            .map(|t| (t.clone(), TestResult::Failed(elapsed, msg.clone())))
            .collect();
    }

    let outcomes = parse_libtest_outcomes(combined_output);
    let per_test_ms = elapsed.as_millis() / tests.len().max(1) as u128;
    let batch_failed = combined_output.contains("test result: FAILED") || combined_output.contains("failures:");
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
                None if teardown_failure && index + 1 == tests.len() => TestResult::Failed(
                    std::time::Duration::from_millis(per_test_ms as u64),
                    extract_libtest_failure_detail(combined_output, "zzzz_incan_harness_teardown_cached_fixtures"),
                ),
                None => TestResult::Failed(
                    elapsed,
                    if combined_output.contains(INCAN_FILE_TEST_MOD) {
                        format!(
                            "Test runner did not report outcome for `{full}`.\nmanifest=`{}` crate=`{}`\nThis may indicate stale/shared test-runner artifacts.\n{combined_output}",
                            manifest_path.display(),
                            crate_name,
                        )
                    } else {
                        format!(
                            "Test runner did not report outcome for `{full}` (see cargo output below)\n{combined_output}"
                        )
                    },
                ),
            };
            (t.clone(), result)
        })
        .collect()
}

/// Run a command and report whether it exceeded the supplied timeout.
fn run_command_with_timeout(mut command: Command, timeout: Option<Duration>) -> std::io::Result<(Output, bool)> {
    let Some(timeout) = timeout else {
        return command.output().map(|output| (output, false));
    };
    let start = Instant::now();
    let mut child = command.spawn()?;
    loop {
        if child.try_wait()?.is_some() {
            return child.wait_with_output().map(|output| (output, false));
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            return child.wait_with_output().map(|output| (output, true));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HarnessPreheatStatus {
    Disabled,
    UpToDate,
    Ran,
    ReusedAfterWait,
}

#[derive(Debug, Clone, Copy)]
struct HarnessPreheatOutcome {
    status: HarnessPreheatStatus,
    elapsed: Duration,
    waited: Duration,
}

struct HarnessPreheatRequest<'a> {
    manifest_path: &'a Path,
    generated_dir: &'a Path,
    project_root: &'a Path,
    shared_target_dir: &'a Path,
    cargo_flags: &'a [String],
    include_cargo_lock: bool,
    jobs: usize,
    timeout: Option<Duration>,
    emit_progress: bool,
}

struct PreheatLockGuard {
    path: PathBuf,
}

impl Drop for PreheatLockGuard {
    /// Remove the cooperative preheat lock file when this writer leaves the critical section.
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Return the age after which an abandoned preheat lock may be reclaimed.
fn stale_preheat_lock_after() -> Duration {
    std::env::var("INCAN_TEST_PREHEAT_STALE_LOCK_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(TEST_HARNESS_PREHEAT_STALE_LOCK_SECS))
}

/// Return whether the recorded preheat fingerprint matches the generated harness inputs.
fn preheat_stamp_matches(stamp_path: &Path, fingerprint: &str) -> bool {
    fs::read_to_string(stamp_path)
        .map(|existing| existing.trim() == fingerprint)
        .unwrap_or(false)
}

/// Try to become the single preheat writer for one generated harness.
fn try_acquire_preheat_lock(lock_path: &Path) -> io::Result<Option<PreheatLockGuard>> {
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    match OpenOptions::new().write(true).create_new(true).open(lock_path) {
        Ok(mut file) => {
            let _ = writeln!(file, "pid={}", std::process::id());
            Ok(Some(PreheatLockGuard {
                path: lock_path.to_path_buf(),
            }))
        }
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(None),
        Err(err) => Err(err),
    }
}

/// Return whether an existing cooperative preheat lock is old enough to discard.
fn preheat_lock_is_stale(lock_path: &Path, stale_after: Duration) -> bool {
    let Ok(metadata) = fs::metadata(lock_path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age >= stale_after)
}

/// Add one generated harness input file to the preheat fingerprint.
fn hash_preheat_file(hasher: &mut Sha256, base: &Path, path: &Path) -> io::Result<()> {
    let relative = path.strip_prefix(base).unwrap_or(path);
    hasher.update(relative.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(fs::read(path)?);
    hasher.update(b"\0");
    Ok(())
}

/// Collect generated Rust source files that define one harness fingerprint.
fn collect_preheat_source_files(root: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_preheat_source_files(&path, files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
    Ok(())
}

/// Compute the fingerprint that determines whether `cargo test --no-run` must be repeated for a harness.
fn compute_generated_harness_preheat_fingerprint(
    generated_dir: &Path,
    cargo_flags: &[String],
    shared_target_dir: &Path,
    include_cargo_lock: bool,
) -> io::Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"incan_test_harness_preheat/1\0");
    hasher.update(shared_target_dir.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    for flag in cargo_flags {
        hasher.update(flag.as_bytes());
        hasher.update(b"\0");
    }
    hash_preheat_file(&mut hasher, generated_dir, &generated_dir.join("Cargo.toml"))?;
    if include_cargo_lock {
        let file_name = "Cargo.lock";
        let path = generated_dir.join(file_name);
        if path.is_file() {
            hash_preheat_file(&mut hasher, generated_dir, &path)?;
        } else {
            hasher.update(file_name.as_bytes());
            hasher.update(b":absent\0");
        }
    }
    let mut source_files = Vec::new();
    collect_preheat_source_files(&generated_dir.join("src"), &mut source_files)?;
    source_files.sort();
    for file in source_files {
        hash_preheat_file(&mut hasher, generated_dir, &file)?;
    }
    Ok(format!(
        "{}{}",
        TEST_HARNESS_PREHEAT_FINGERPRINT_FILE,
        hex::encode(hasher.finalize())
    ))
}

/// Build the Cargo command used by both harness preheat and actual harness execution.
fn cargo_test_command(
    manifest_path: &Path,
    cargo_flags: &[String],
    jobs: usize,
    shared_target_dir: &Path,
    project_root: &Path,
    no_run: bool,
    no_capture: bool,
) -> Command {
    let mut command = Command::new("cargo");
    command.arg("test");
    if no_run {
        command.arg("--no-run");
    }
    if jobs > 1 {
        command.arg("--jobs");
        command.arg(jobs.to_string());
    }
    command.arg("--manifest-path");
    command.arg(manifest_path);
    for flag in cargo_flags {
        command.arg(flag);
    }
    if !no_run {
        // Batched per-file execution shares one process across all generated #[test] fns.
        // Force deterministic single-thread libtest execution to preserve historical
        // isolation assumptions for tests that use shared global runtime state.
        command.arg("--");
        command.arg("--test-threads=1");
        if no_capture {
            command.arg("--nocapture");
        }
    }
    command.env("CARGO_TARGET_DIR", shared_target_dir);
    // Keep runtime-relative fixture paths anchored to the caller's project, not the generated test crate.
    command.current_dir(project_root);
    command
}

/// Run `cargo test --no-run` for one generated harness and return its elapsed time.
fn run_generated_harness_preheat(request: &HarnessPreheatRequest<'_>) -> Result<Duration, String> {
    let start = Instant::now();
    let command = cargo_test_command(
        request.manifest_path,
        request.cargo_flags,
        request.jobs,
        request.shared_target_dir,
        request.project_root,
        true,
        false,
    );
    let (output, timed_out) = run_command_with_timeout(command, request.timeout)
        .map_err(|err| format!("failed to run cargo test --no-run: {err}"))?;
    if timed_out {
        let timeout = request
            .timeout
            .map(|timeout| format!("{:.3}s", timeout.as_secs_f64()))
            .unwrap_or_else(|| "configured timeout".to_string());
        return Err(format!("cargo test --no-run timed out after {timeout}"));
    }
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("cargo test --no-run failed:\n{stdout}\n{stderr}"));
    }
    Ok(start.elapsed())
}

/// Preheat one generated harness if its recorded fingerprint is missing or stale.
fn preheat_generated_harness_if_needed(request: HarnessPreheatRequest<'_>) -> Result<HarnessPreheatOutcome, String> {
    let start = Instant::now();
    if !test_preheat_enabled() {
        return Ok(HarnessPreheatOutcome {
            status: HarnessPreheatStatus::Disabled,
            elapsed: start.elapsed(),
            waited: Duration::ZERO,
        });
    }

    let fingerprint = compute_generated_harness_preheat_fingerprint(
        request.generated_dir,
        request.cargo_flags,
        request.shared_target_dir,
        request.include_cargo_lock,
    )
    .map_err(|err| format!("failed to fingerprint generated test harness: {err}"))?;
    let stamp_path = request.generated_dir.join(TEST_HARNESS_PREHEAT_FINGERPRINT_FILE);
    if preheat_stamp_matches(&stamp_path, &fingerprint) {
        return Ok(HarnessPreheatOutcome {
            status: HarnessPreheatStatus::UpToDate,
            elapsed: start.elapsed(),
            waited: Duration::ZERO,
        });
    }

    if request.emit_progress {
        println!(
            "preheating generated Rust test harness {}",
            request.generated_dir.display()
        );
        let _ = io::stdout().flush();
    }

    let lock_path = request.generated_dir.join(TEST_HARNESS_PREHEAT_LOCK_FILE);
    let stale_after = stale_preheat_lock_after();
    let wait_start = Instant::now();
    let mut announced_wait = false;
    let lock = loop {
        if preheat_stamp_matches(&stamp_path, &fingerprint) {
            let waited = wait_start.elapsed();
            return Ok(HarnessPreheatOutcome {
                status: HarnessPreheatStatus::ReusedAfterWait,
                elapsed: start.elapsed(),
                waited,
            });
        }

        match try_acquire_preheat_lock(&lock_path) {
            Ok(Some(lock)) => break lock,
            Ok(None) => {
                if preheat_lock_is_stale(&lock_path, stale_after) {
                    let _ = fs::remove_file(&lock_path);
                    continue;
                }
                if request.emit_progress && !announced_wait && wait_start.elapsed() >= Duration::from_secs(1) {
                    println!("waiting for another incan test preheat to finish");
                    let _ = io::stdout().flush();
                    announced_wait = true;
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(err) => return Err(format!("failed to acquire preheat lock {}: {err}", lock_path.display())),
        }
    };

    if preheat_stamp_matches(&stamp_path, &fingerprint) {
        drop(lock);
        return Ok(HarnessPreheatOutcome {
            status: HarnessPreheatStatus::UpToDate,
            elapsed: start.elapsed(),
            waited: wait_start.elapsed(),
        });
    }

    run_generated_harness_preheat(&request)?;
    fs::write(&stamp_path, &fingerprint)
        .map_err(|err| format!("failed to write preheat fingerprint {}: {err}", stamp_path.display()))?;
    drop(lock);

    Ok(HarnessPreheatOutcome {
        status: HarnessPreheatStatus::Ran,
        elapsed: start.elapsed(),
        waited: wait_start.elapsed(),
    })
}

/// Return the stable diagnostic label for a preheat outcome.
fn preheat_status_label(status: HarnessPreheatStatus) -> &'static str {
    match status {
        HarnessPreheatStatus::Disabled => "disabled",
        HarnessPreheatStatus::UpToDate => "up-to-date",
        HarnessPreheatStatus::Ran => "ran",
        HarnessPreheatStatus::ReusedAfterWait => "reused-after-wait",
    }
}

/// Run every collected test in `tests` that lives in the same `.incn` file with **one** `cargo test` invocation (#271).
///
/// Returns an empty vector when `tests` is empty. Otherwise every entry must share the same [`TestInfo::file_path`].
/// Skip/xfail handling stays in [`super::run_tests`].
#[allow(clippy::too_many_arguments)]
pub(super) fn run_file_tests_batch(
    tests: &[TestInfo],
    conftest_files_by_file: &HashMap<PathBuf, Vec<PathBuf>>,
    prep_cache: &mut HashMap<String, Arc<PreparedTestFile>>,
    cargo_policy: &CargoPolicy,
    cargo_features: &[String],
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    options: TestExecutionOptions,
) -> Vec<(TestInfo, TestResult)> {
    if tests.is_empty() {
        return Vec::new();
    }

    let start = Instant::now();
    let first = &tests[0];

    // ---- Context: load test source, discover manifest, parse and vocab-desugar the test file ----
    let mut source_parts = Vec::new();
    let mut sources_by_file = Vec::new();
    let mut seen_conftests = BTreeSet::new();
    let mut seen_files = BTreeSet::new();
    for test in tests {
        if !seen_files.insert(test.file_path.clone()) {
            continue;
        }
        if let Some(conftest_files) = conftest_files_by_file.get(&test.file_path) {
            for conftest in conftest_files {
                if !seen_conftests.insert(conftest.clone()) {
                    continue;
                }
                match fs::read_to_string(conftest) {
                    Ok(source) => source_parts.push(source),
                    Err(err) => {
                        let message = format!("Failed to read conftest {}: {}", conftest.display(), err);
                        return tests
                            .iter()
                            .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), message.clone())))
                            .collect();
                    }
                }
            }
        }
        match fs::read_to_string(&test.file_path) {
            Ok(source) => {
                sources_by_file.push((test.file_path.clone(), source.clone()));
                source_parts.push(source);
            }
            Err(e) => {
                return tests
                    .iter()
                    .map(|t| {
                        (
                            t.clone(),
                            TestResult::Failed(start.elapsed(), format!("Failed to read file: {}", e)),
                        )
                    })
                    .collect();
            }
        }
    }
    let source = source_parts.join("\n");

    let manifest = match ProjectManifest::discover(first.file_path.parent().unwrap_or_else(|| Path::new("."))) {
        Ok(manifest) => manifest,
        Err(err) => {
            return tests
                .iter()
                .map(|t| {
                    (
                        t.clone(),
                        TestResult::Failed(start.elapsed(), format!("Manifest error: {}", err)),
                    )
                })
                .collect();
        }
    };
    let library_manifest_index = manifest
        .as_ref()
        .map(LibraryManifestIndex::from_project_manifest)
        .unwrap_or_default();
    let library_imported_vocab = library_manifest_index.library_imported_vocab();
    let library_imported_dsl_surfaces = library_manifest_index.library_imported_dsl_surfaces();

    if batch_has_cross_file_top_level_collision(&sources_by_file, Some(&library_imported_vocab)) {
        let mut split_results = Vec::new();
        for file_path in seen_files {
            let file_tests = tests
                .iter()
                .filter(|test| test.file_path == file_path)
                .cloned()
                .collect::<Vec<_>>();
            split_results.extend(run_file_tests_batch(
                &file_tests,
                conftest_files_by_file,
                prep_cache,
                cargo_policy,
                cargo_features,
                cargo_no_default_features,
                cargo_all_features,
                options,
            ));
        }
        return split_results;
    }

    let tokens = match lexer::lex(&source) {
        Ok(t) => t,
        Err(e) => {
            return tests
                .iter()
                .map(|t| {
                    (
                        t.clone(),
                        TestResult::Failed(start.elapsed(), format!("Lexer error: {:?}", e)),
                    )
                })
                .collect();
        }
    };

    let path_display = first.file_path.to_string_lossy();
    let mut ast = match parser::parse_with_context_and_surfaces(
        &tokens,
        Some(path_display.as_ref()),
        Some(&library_imported_vocab),
        Some(&library_imported_dsl_surfaces),
    ) {
        Ok(a) => a,
        Err(e) => {
            return tests
                .iter()
                .map(|t| {
                    (
                        t.clone(),
                        TestResult::Failed(start.elapsed(), format!("Parser error: {:?}", e)),
                    )
                })
                .collect();
        }
    };
    if let Err(errors) =
        vocab_desugar_pass::desugar_program_vocab_blocks(&mut ast, Some(path_display.as_ref()), &library_manifest_index)
    {
        return tests
            .iter()
            .map(|t| {
                (
                    t.clone(),
                    TestResult::Failed(start.elapsed(), format!("Vocab desugar error: {:?}", errors)),
                )
            })
            .collect();
    }
    let mut runner_ast = ast_with_inline_test_declarations(&ast);
    normalize_runner_assert_statements(&mut runner_ast);
    prune_shadowed_fixture_declarations(&mut runner_ast);
    dedupe_import_declarations(&mut runner_ast);
    let fixture_teardowns = match split_yield_fixture_declarations(&mut runner_ast) {
        Ok(teardowns) => teardowns,
        Err(message) => {
            return tests
                .iter()
                .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), message.clone())))
                .collect();
        }
    };

    let cargo_feature_selection = CargoFeatureSelection {
        cargo_features: cargo_features.to_vec(),
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();

    // ---- Context: resolve project paths and collect transitive Incan modules for the test ----
    let project_root = manifest
        .as_ref()
        .map(|m| m.project_root().to_path_buf())
        .unwrap_or_else(|| infer_project_root_without_manifest(&first.file_path));
    let project_root = absolute_project_root(&project_root);
    let source_root = common::resolve_source_root(&project_root, manifest.as_ref());
    let source_modules = match collect_source_modules_for_test(
        &runner_ast,
        &source_root,
        Some(&library_imported_vocab),
        Some(&library_imported_dsl_surfaces),
        Some(&library_manifest_index),
    ) {
        Ok(m) => m,
        Err(e) => {
            return tests
                .iter()
                .map(|t| {
                    (
                        t.clone(),
                        TestResult::Failed(start.elapsed(), format!("Failed to collect source modules: {}", e)),
                    )
                })
                .collect();
        }
    };

    // ---- Context: session prep cache — reuse deps / lock / rust-inspect when key matches ----
    let cache_key = compute_test_prep_cache_key(
        &first.file_path,
        &source,
        &source_modules,
        manifest.as_ref(),
        &cargo_feature_selection,
        cargo_policy,
    );

    let prepared: Arc<PreparedTestFile> = if let Some(hit) = prep_cache.get(&cache_key) {
        Arc::clone(hit)
    } else {
        // ---- Context: cold prep — inline imports, resolve and merge Cargo deps, lock + rust-inspect workspace ----
        let module_for_imports = ParsedModule {
            name: "test".to_string(),
            path_segments: vec!["test".to_string()],
            file_path: first.file_path.clone(),
            source: source.clone(),
            ast: runner_ast.clone(),
        };

        let mut dependency_modules: Vec<ParsedModule> = Vec::with_capacity(1 + source_modules.len());
        dependency_modules.push(module_for_imports);
        dependency_modules.extend(source_modules.iter().map(|m| ParsedModule {
            name: m.name.clone(),
            path_segments: m.path_segments.clone(),
            file_path: m.file_path.clone(),
            source: m.source.clone(),
            ast: m.ast.clone(),
        }));

        let mut inline_imports = Vec::new();
        for module in &dependency_modules {
            inline_imports.extend(common::collect_inline_rust_imports(module, true));
        }

        let project_requirements =
            match common::collect_project_requirements(&dependency_modules, &library_manifest_index) {
                Ok(requirements) => requirements,
                Err(err) => {
                    return tests
                        .iter()
                        .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), err.message.clone())))
                        .collect();
                }
            };

        let mut resolved =
            match resolve_dependencies(manifest.as_ref(), &inline_imports, true, &cargo_feature_selection) {
                Ok(resolved) => resolved,
                Err(errors) => {
                    let mut sources = HashMap::new();
                    sources.insert(first.file_path.clone(), source.clone());
                    let mut msg = String::new();
                    for err in &errors {
                        msg.push_str(&common::format_dependency_error(err, &sources));
                    }
                    return tests
                        .iter()
                        .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), msg.clone())))
                        .collect();
                }
            };
        if let Err(err) = common::merge_project_requirement_dependencies(&mut resolved, &project_requirements) {
            return tests
                .iter()
                .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), err.message.clone())))
                .collect();
        }

        let project_name = manifest
            .as_ref()
            .and_then(|m| m.project.as_ref().and_then(|p| p.name.clone()))
            .or_else(|| {
                first
                    .file_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "incan_test".to_string());
        let lock_payload = match commands::resolve_lock_payload(commands::LockResolutionRequest {
            project_root: &project_root,
            project_name: &project_name,
            manifest: manifest.as_ref(),
            resolved: &resolved,
            project_requirements: &project_requirements,
            cargo_features: &cargo_feature_selection,
            cargo_policy,
        }) {
            Ok(payload) => payload,
            Err(err) => {
                return tests
                    .iter()
                    .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), err.message.clone())))
                    .collect();
            }
        };

        #[cfg(feature = "rust_inspect")]
        let rust_inspect_manifest_dir = {
            let metadata_query_paths = collect_rust_inspect_query_paths(&dependency_modules);
            let mut rust_inspect_requirements = project_requirements.clone();
            rust_inspect_requirements.stdlib_features = merge_rust_inspect_stdlib_features(
                prep_cache
                    .values()
                    .map(|prepared| prepared.project_requirements.stdlib_features.as_slice()),
                &project_requirements.stdlib_features,
            );
            let rust_inspect_manifest_dir = match ensure_rust_inspect_workspace(
                &project_root,
                &project_name,
                manifest
                    .as_ref()
                    .and_then(|m| m.build.as_ref().and_then(|build| build.rust_edition.clone())),
                &resolved,
                &rust_inspect_requirements,
                lock_payload.clone(),
            ) {
                Ok(dir) => dir,
                Err(err) => {
                    return tests
                        .iter()
                        .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), err.message.clone())))
                        .collect();
                }
            };
            if let Err(err) = prewarm_rust_inspect_workspace(&rust_inspect_manifest_dir, &metadata_query_paths) {
                return tests
                    .iter()
                    .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), err.message.clone())))
                    .collect();
            }
            rust_inspect_manifest_dir
        };

        let prepared = PreparedTestFile {
            library_manifest_index,
            ast: runner_ast,
            fixture_teardowns,
            source_modules,
            project_root,
            resolved,
            project_requirements,
            lock_payload,
            #[cfg(feature = "rust_inspect")]
            rust_inspect_manifest_dir,
        };
        let arc = Arc::new(prepared);
        prep_cache.insert(cache_key, Arc::clone(&arc));
        arc
    };

    // ---- Codegen + unified file harness (library + #[cfg(test)] module) ----
    let mut codegen = IrCodegen::new();
    codegen.set_preserve_dependency_public_items(false);
    codegen.set_library_manifest_index(prepared.library_manifest_index.clone());
    #[cfg(feature = "rust_inspect")]
    {
        codegen.set_rust_inspect_manifest_dir(prepared.rust_inspect_manifest_dir.clone());
    }

    for module in &prepared.source_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }
    let fixtures = collect_fixture_execution_info(&prepared.ast, &prepared.fixture_teardowns);
    codegen.set_externally_reachable_items(collect_harness_entrypoints(tests, &fixtures));

    let batch_file_paths = tests.iter().map(|test| test.file_path.clone()).collect::<Vec<_>>();
    let dir_suffix = file_batch_dir_suffix(&batch_file_paths);
    let runner_crate_name = runner_crate_name_for_batch_suffix(&dir_suffix);
    let temp_dir = format!("target/incan_tests/{}", dir_suffix);
    let temp_dir_path = PathBuf::from(&temp_dir);
    let manifest_path = if temp_dir_path.is_absolute() {
        temp_dir_path.join("Cargo.toml")
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(&temp_dir).join("Cargo.toml")
    } else {
        temp_dir_path.join("Cargo.toml")
    };

    let mut generator = ProjectGenerator::new(&temp_dir, &runner_crate_name, false);
    generator.set_stdlib_features(prepared.project_requirements.stdlib_features.clone());
    generator.set_cargo_lock_payload(prepared.lock_payload.clone());
    let cargo_flags = common::cargo_command_flags(cargo_policy, &cargo_feature_selection);
    generator.set_cargo_policy_flags(cargo_flags.clone());

    let gen_err = |msg: String| {
        tests
            .iter()
            .map(|t| (t.clone(), TestResult::Failed(start.elapsed(), msg.clone())))
            .collect::<Vec<_>>()
    };

    let runner_dependencies =
        match merge_test_runner_dependencies(&prepared.resolved.dependencies, &prepared.resolved.dev_dependencies) {
            Ok(deps) => deps,
            Err(message) => return gen_err(message),
        };
    generator.set_include_dev_dependencies(false);
    generator.set_dependencies(runner_dependencies);
    generator.set_dev_dependencies(Vec::new());

    let generated_changed = if prepared.source_modules.is_empty() {
        let rust_code = match codegen.try_generate(&prepared.ast) {
            Ok(code) => code,
            Err(e) => return gen_err(format!("Code generation error: {}", e)),
        };
        let rust_code = inject_file_test_harness(&rust_code, tests, &prepared.project_root, &fixtures);
        match generator.generate(&rust_code) {
            Ok(changed) => changed,
            Err(e) => return gen_err(format!("Failed to generate project: {}", e)),
        }
    } else {
        let module_paths: Vec<Vec<String>> = prepared
            .source_modules
            .iter()
            .map(|m| m.path_segments.clone())
            .collect();
        let (main_code, rust_modules) = match codegen.try_generate_multi_file_nested(&prepared.ast, &module_paths) {
            Ok(result) => result,
            Err(e) => return gen_err(format!("Code generation error: {}", e)),
        };
        let main_code = inject_file_test_harness(&main_code, tests, &prepared.project_root, &fixtures);
        match generator.generate_nested(&main_code, &rust_modules) {
            Ok(changed) => changed,
            Err(e) => return gen_err(format!("Failed to generate project: {}", e)),
        }
    };

    let shared_target_dir = shared_cargo_target_dir(&prepared.project_root);
    let generated_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let preheat_outcome = match preheat_generated_harness_if_needed(HarnessPreheatRequest {
        manifest_path: &manifest_path,
        generated_dir,
        project_root: &prepared.project_root,
        shared_target_dir: &shared_target_dir,
        cargo_flags: &cargo_flags,
        include_cargo_lock: prepared.lock_payload.is_some(),
        jobs: options.jobs,
        timeout: options.timeout,
        emit_progress: options.emit_progress,
    }) {
        Ok(outcome) => outcome,
        Err(message) => return gen_err(format!("Failed to preheat generated test harness: {message}")),
    };
    if options.verbose && options.emit_progress {
        println!(
            "preheat phase: {} in {:.2}s (wait {:.2}s, generated changed: {})",
            preheat_status_label(preheat_outcome.status),
            preheat_outcome.elapsed.as_secs_f64(),
            preheat_outcome.waited.as_secs_f64(),
            generated_changed
        );
    }

    let command = cargo_test_command(
        &manifest_path,
        &cargo_flags,
        options.jobs,
        &shared_target_dir,
        &prepared.project_root,
        false,
        options.no_capture,
    );

    let (output, timed_out) = match run_command_with_timeout(command, options.timeout) {
        Ok(result) => result,
        Err(e) => {
            return tests
                .iter()
                .map(|t| {
                    (
                        t.clone(),
                        TestResult::Failed(start.elapsed(), format!("Failed to run cargo test: {}", e)),
                    )
                })
                .collect();
        }
    };

    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    if options.no_capture && !combined.trim().is_empty() {
        print!("{combined}");
    }

    if timed_out {
        let message = match options.timeout {
            Some(timeout) => format!("test batch timed out after {:.3}s", timeout.as_secs_f64()),
            None => "test batch timed out".to_string(),
        };
        return tests
            .iter()
            .map(|t| (t.clone(), TestResult::Failed(elapsed, message.clone())))
            .collect();
    }

    let compile_failed = !output.status.success()
        && (combined.contains("could not compile")
            || combined.contains("error: could not compile")
            || combined.contains("error[E")
            || combined.contains("error: aborting due to"));

    if compile_failed {
        return map_batch_results(
            tests,
            &combined,
            elapsed,
            true,
            &combined,
            &manifest_path,
            &runner_crate_name,
        );
    }

    map_batch_results(tests, &combined, elapsed, false, "", &manifest_path, &runner_crate_name)
}

/// Inject a `fn main()` into generated Rust code that calls the test function (legacy single-case `cargo run` path).
///
/// Kept for unit tests only; the runner uses [`inject_file_test_harness`] + `cargo test` per source file (#271).
#[cfg(test)]
fn inject_test_main(
    rust_code: &str,
    function_name: &str,
    parametrize: Option<&super::types::ParametrizeCall>,
) -> String {
    let call = if let Some(pc) = parametrize {
        format!("    {}({});", function_name, pc.rust_args())
    } else {
        format!("    {}();", function_name)
    };
    format!("{}\nfn main() {{\n{}\n}}\n", rust_code, call)
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

/// Extract a panic message from stdout.
fn extract_panic_message(stdout: &str) -> String {
    let mut in_panic = false;
    let mut msg = String::new();

    for line in stdout.lines() {
        if line.contains("panicked at") {
            in_panic = true;
            msg.push_str(line.trim());
            msg.push('\n');
        } else if in_panic && line.starts_with("  ") {
            msg.push_str(line);
            msg.push('\n');
        } else if in_panic && line.is_empty() {
            break;
        }
    }

    if msg.is_empty() { stdout.to_string() } else { msg }
}

#[cfg(test)]
mod tests {
    use super::super::types::ParametrizeCall;
    use super::*;
    use std::path::Path;

    #[test]
    fn shared_target_dir_stays_under_project_target() {
        let target_dir = shared_cargo_target_dir(Path::new("/tmp/incan_project"));
        assert_eq!(target_dir, PathBuf::from("/tmp/incan_project/target"));
    }

    #[test]
    fn parse_isolated_target_env_requires_truthy_values() {
        assert!(parse_isolated_target_env(Some("1")));
        assert!(parse_isolated_target_env(Some("true")));
        assert!(parse_isolated_target_env(Some(" yes ")));
        assert!(parse_isolated_target_env(Some("on")));
        assert!(!parse_isolated_target_env(None));
        assert!(!parse_isolated_target_env(Some("0")));
        assert!(!parse_isolated_target_env(Some("false")));
        assert!(!parse_isolated_target_env(Some("off")));
        assert!(!parse_isolated_target_env(Some("")));
    }

    #[test]
    fn parse_test_preheat_env_defaults_to_enabled() {
        assert!(parse_test_preheat_env(None));
        assert!(parse_test_preheat_env(Some("1")));
        assert!(parse_test_preheat_env(Some("true")));
        assert!(!parse_test_preheat_env(Some("0")));
        assert!(!parse_test_preheat_env(Some("false")));
        assert!(!parse_test_preheat_env(Some(" off ")));
    }

    #[test]
    fn generated_harness_preheat_fingerprint_changes_when_source_changes() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join(format!("incan_preheat_fingerprint_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(temp_dir.join("src"))?;
        fs::write(
            temp_dir.join("Cargo.toml"),
            "[package]\nname = \"preheat_test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(temp_dir.join("src").join("lib.rs"), "pub fn value() -> i64 { 1 }\n")?;

        let first = compute_generated_harness_preheat_fingerprint(&temp_dir, &[], &temp_dir.join("target"), false)?;
        fs::write(temp_dir.join("src").join("lib.rs"), "pub fn value() -> i64 { 2 }\n")?;
        let second = compute_generated_harness_preheat_fingerprint(&temp_dir, &[], &temp_dir.join("target"), false)?;

        assert_ne!(first, second);
        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn generated_harness_preheat_fingerprint_includes_cargo_flags() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join(format!("incan_preheat_flags_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(temp_dir.join("src"))?;
        fs::write(
            temp_dir.join("Cargo.toml"),
            "[package]\nname = \"preheat_flags\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(temp_dir.join("src").join("lib.rs"), "pub fn value() -> i64 { 1 }\n")?;

        let base = compute_generated_harness_preheat_fingerprint(&temp_dir, &[], &temp_dir.join("target"), false)?;
        let locked = compute_generated_harness_preheat_fingerprint(
            &temp_dir,
            &["--locked".to_string()],
            &temp_dir.join("target"),
            false,
        )?;

        assert_ne!(base, locked);
        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn preheat_lock_guard_removes_lock_on_drop() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join(format!("incan_preheat_lock_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir)?;
        let lock_path = temp_dir.join(TEST_HARNESS_PREHEAT_LOCK_FILE);

        let guard = try_acquire_preheat_lock(&lock_path)?.expect("first lock acquisition should win");
        assert!(lock_path.is_file());
        assert!(try_acquire_preheat_lock(&lock_path)?.is_none());
        drop(guard);
        assert!(!lock_path.exists());

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn inject_main_plain_test() {
        let rust = "fn test_add() { assert_eq!(1 + 1, 2); }";
        let result = inject_test_main(rust, "test_add", None);
        assert!(result.contains("fn main()"), "should inject fn main()");
        assert!(result.contains("test_add();"), "should call test function");
        assert!(!result.contains("test_add(1"), "plain test has no args");
    }

    #[test]
    fn inject_main_parametrized_int_args() {
        let rust = "fn test_add(x: i64, y: i64, expected: i64) { assert_eq!(x + y, expected); }";
        let pc = ParametrizeCall {
            display_id: "test_add[1-2-3]".to_string(),
            argument_names: vec!["x".to_string(), "y".to_string(), "expected".to_string()],
            rust_arguments: vec!["1".to_string(), "2".to_string(), "3".to_string()],
            parameters: vec!["1".to_string(), "2".to_string(), "3".to_string()],
        };
        let result = inject_test_main(rust, "test_add", Some(&pc));
        assert!(result.contains("fn main()"), "should inject fn main()");
        assert!(result.contains("test_add(1, 2, 3);"), "should call with int args");
    }

    #[test]
    fn inject_main_parametrized_string_args() {
        let rust = "fn test_len(input: String, expected: i64) {}";
        let pc = ParametrizeCall {
            display_id: "test_len[hello-5]".to_string(),
            argument_names: vec!["input".to_string(), "expected".to_string()],
            rust_arguments: vec!["\"hello\".to_string()".to_string(), "5".to_string()],
            parameters: vec!["hello".to_string(), "5".to_string()],
        };
        let result = inject_test_main(rust, "test_len", Some(&pc));
        assert!(
            result.contains("test_len(\"hello\".to_string(), 5);"),
            "should call with string + int args"
        );
    }

    #[test]
    fn inject_main_parametrized_negative_args() {
        let rust = "fn test_sub(a: i64, b: i64, expected: i64) {}";
        let pc = ParametrizeCall {
            display_id: "test_sub[-1-1-0]".to_string(),
            argument_names: vec!["a".to_string(), "b".to_string(), "expected".to_string()],
            rust_arguments: vec!["-1".to_string(), "1".to_string(), "0".to_string()],
            parameters: vec!["-1".to_string(), "1".to_string(), "0".to_string()],
        };
        let result = inject_test_main(rust, "test_sub", Some(&pc));
        assert!(
            result.contains("test_sub(-1, 1, 0);"),
            "should call with negative int args"
        );
    }

    #[test]
    fn cross_file_batch_collision_detects_duplicate_top_level_model_names() {
        let sources = vec![
            (
                PathBuf::from("tests/test_a.incn"),
                "model Order:\n  id: int\n\ndef test_a() -> None:\n  pass\n".to_string(),
            ),
            (
                PathBuf::from("tests/test_b.incn"),
                "model Order:\n  id: int\n\ndef test_b() -> None:\n  pass\n".to_string(),
            ),
        ];

        assert!(batch_has_cross_file_top_level_collision(&sources, None));
    }

    #[test]
    fn cross_file_batch_collision_allows_distinct_top_level_names() {
        let sources = vec![
            (
                PathBuf::from("tests/test_a.incn"),
                "model OrderA:\n  id: int\n\ndef test_a() -> None:\n  pass\n".to_string(),
            ),
            (
                PathBuf::from("tests/test_b.incn"),
                "model OrderB:\n  id: int\n\ndef test_b() -> None:\n  pass\n".to_string(),
            ),
        ];

        assert!(!batch_has_cross_file_top_level_collision(&sources, None));
    }

    #[test]
    fn prep_cache_key_stable_for_identical_inputs() {
        let cargo = CargoFeatureSelection {
            cargo_features: vec!["serde".to_string()],
            cargo_no_default_features: false,
            cargo_all_features: false,
        }
        .normalized();
        let mods: Vec<ParsedModule> = Vec::new();
        let policy = CargoPolicy::default();
        let k1 = compute_test_prep_cache_key(Path::new("tests/test_x.incn"), "source a", &mods, None, &cargo, &policy);
        let k2 = compute_test_prep_cache_key(Path::new("tests/test_x.incn"), "source a", &mods, None, &cargo, &policy);
        assert_eq!(k1, k2);
        assert!(k1.starts_with("v1:"));
    }

    #[test]
    fn prep_cache_key_changes_when_test_source_changes() {
        let cargo = CargoFeatureSelection::default().normalized();
        let mods: Vec<ParsedModule> = Vec::new();
        let policy = CargoPolicy::default();
        let k1 = compute_test_prep_cache_key(Path::new("t.incn"), "a", &mods, None, &cargo, &policy);
        let k2 = compute_test_prep_cache_key(Path::new("t.incn"), "b", &mods, None, &cargo, &policy);
        assert_ne!(k1, k2);
    }

    #[test]
    fn prep_cache_key_includes_cargo_policy() {
        let cargo = CargoFeatureSelection::default().normalized();
        let mods: Vec<ParsedModule> = Vec::new();
        let base = CargoPolicy::default();
        let locked = CargoPolicy::explicit(false, true, false, Vec::new());
        let offline = CargoPolicy::explicit(true, false, false, Vec::new());
        let extra = CargoPolicy::explicit(false, false, false, vec!["--timings".to_string()]);

        let k_base = compute_test_prep_cache_key(Path::new("t.incn"), "x", &mods, None, &cargo, &base);
        let k_locked = compute_test_prep_cache_key(Path::new("t.incn"), "x", &mods, None, &cargo, &locked);
        let k_offline = compute_test_prep_cache_key(Path::new("t.incn"), "x", &mods, None, &cargo, &offline);
        let k_extra = compute_test_prep_cache_key(Path::new("t.incn"), "x", &mods, None, &cargo, &extra);

        assert_ne!(k_base, k_locked);
        assert_ne!(k_base, k_offline);
        assert_ne!(k_base, k_extra);
    }

    #[test]
    fn merge_rust_inspect_stdlib_features_unions_and_sorts() {
        let existing = [
            vec!["json".to_string(), "async".to_string()],
            vec!["web".to_string(), "async".to_string()],
        ];
        let merged = merge_rust_inspect_stdlib_features(
            existing.iter().map(|set| set.as_slice()),
            &["serde".to_string(), "json".to_string()],
        );
        assert_eq!(
            merged,
            vec![
                "async".to_string(),
                "json".to_string(),
                "serde".to_string(),
                "web".to_string()
            ]
        );
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
    fn runner_crate_name_is_derived_from_batch_suffix() {
        let name = runner_crate_name_for_batch_suffix("batch_76001490ba86f677");
        assert_eq!(name, "test_runner_76001490ba86f677");
    }

    #[test]
    fn inject_file_test_harness_emits_tests_module() {
        let rust = "fn test_a() {}\nfn test_b() {}\n";
        let tests = vec![
            TestInfo {
                file_path: PathBuf::from("t.incn"),
                function_name: "test_a".to_string(),
                markers: vec![],
                required_fixtures: vec![],
                parameter_names: vec![],
                timeout: None,
                parametrize_call: None,
            },
            TestInfo {
                file_path: PathBuf::from("t.incn"),
                function_name: "test_b".to_string(),
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
}
