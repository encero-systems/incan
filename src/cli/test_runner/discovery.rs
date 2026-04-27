use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::frontend::ast::{
    BinaryOp, CallArg, Declaration, DecoratorArg, DecoratorArgValue, Expr, ListEntry, Literal, Program, Spanned,
    Statement, UnaryOp,
};
use crate::frontend::ast_walk::any_expr_in_body;
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::testing_markers::{
    TestingMarkerKind, TestingMarkerSemantics, load_testing_marker_semantics, resolve_testing_marker_kind,
};
use crate::frontend::{lexer, parser};
use crate::manifest::ProjectManifest;

use super::types::{DiscoveryResult, FixtureInfo, FixtureScope, ParametrizeCase, TestInfo, TestMarker};

/// Return whether `path` uses the conventional standalone test-file naming scheme.
fn is_named_test_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    (name.starts_with("test_") || name.ends_with("_test.incn")) && name.ends_with(".incn")
}

/// Return whether `path` is an Incan source file, regardless of whether it is a conventional test file.
fn is_incan_source_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("incn")
}

/// Cheap pre-parse filter for inline test modules.
///
/// Directory discovery can see many production files.  This check avoids parsing ordinary `.incn` files that cannot
/// contain RFC 018 inline tests while still requiring a real parser-confirmed `Declaration::TestModule` before the file
/// becomes a test target.
fn source_may_contain_inline_test_module(source: &str) -> bool {
    source.contains("module tests")
}

/// Parse a non-test source file just far enough to prove it contains a real RFC 018 inline test module.
fn file_has_inline_test_module(path: &Path) -> bool {
    if !is_incan_source_file(path) || is_named_test_file(path) {
        return false;
    }

    let Ok(source) = fs::read_to_string(path) else {
        return false;
    };
    if !source_may_contain_inline_test_module(&source) {
        return false;
    }

    let Ok(tokens) = lexer::lex(&source) else {
        return false;
    };
    let path_display = path.to_string_lossy();
    let Ok(ast) = parser::parse_with_module_path(&tokens, Some(path_display.as_ref())) else {
        return false;
    };

    ast.declarations
        .iter()
        .any(|decl| matches!(decl.node, Declaration::TestModule(_)))
}

/// Build a lightweight [`Program`] wrapper around a declaration slice so existing import-alias collection stays shared.
fn program_for_decls(declarations: Vec<Spanned<Declaration>>) -> Program {
    Program {
        declarations,
        rust_module_path: None,
        warnings: Vec::new(),
    }
}

const BUILTIN_FIXTURES: &[&str] = &["tmp_path", "tmp_workdir", "env"];

/// Collection-time inputs available to conditional test markers.
#[derive(Debug, Clone)]
pub(crate) struct CollectionEvalContext {
    platform: String,
    features: BTreeSet<String>,
}

impl CollectionEvalContext {
    /// Create collection-time evaluation inputs from CLI-provided testing features.
    pub(crate) fn new(features: BTreeSet<String>) -> Self {
        Self {
            platform: std::env::consts::OS.to_string(),
            features,
        }
    }
}

impl Default for CollectionEvalContext {
    /// Use the host platform with no enabled testing features.
    fn default() -> Self {
        Self::new(BTreeSet::new())
    }
}

/// Discover test files in a directory.
pub fn discover_test_files(path: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    if path.is_file() {
        if is_named_test_file(path) || file_has_inline_test_module(path) {
            files.push(path.to_path_buf());
        }
    } else if path.is_dir()
        && let Ok(entries) = fs::read_dir(path)
    {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                let name = entry_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with('.') && name != "target" && name != "node_modules" {
                    files.extend(discover_test_files(&entry_path));
                }
            } else {
                if is_named_test_file(&entry_path) || file_has_inline_test_module(&entry_path) {
                    files.push(entry_path);
                }
            }
        }
    }

    files.sort();
    files
}

/// Discover both tests and fixtures in a file.
pub fn discover_tests_and_fixtures(file_path: &Path) -> Result<DiscoveryResult, String> {
    discover_tests_and_fixtures_with_context(file_path, &[], &[], &[], &CollectionEvalContext::default())
}

/// Discover test declarations using inherited conftest fixtures and marker registries.
pub(crate) fn discover_tests_and_fixtures_with_context(
    file_path: &Path,
    visible_fixture_names: &[String],
    inherited_marks: &[String],
    inherited_known_markers: &[String],
    eval_context: &CollectionEvalContext,
) -> Result<DiscoveryResult, String> {
    let source = fs::read_to_string(file_path).map_err(|e| format!("Failed to read file: {}", e))?;

    let tokens = lexer::lex(&source).map_err(|e| format!("Lexer error: {:?}", e))?;

    let path_display = file_path.to_string_lossy();
    let manifest = ProjectManifest::discover(file_path.parent().unwrap_or_else(|| Path::new(".")))
        .map_err(|e| format!("Manifest error: {}", e))?;
    let library_manifest_index = manifest
        .as_ref()
        .map(LibraryManifestIndex::from_project_manifest)
        .unwrap_or_default();
    let library_imported_vocab = library_manifest_index.library_imported_vocab();
    let ast = parser::parse_with_context(&tokens, Some(path_display.as_ref()), Some(&library_imported_vocab))
        .map_err(|e| format!("Parser error: {:?}", e))?;

    let is_named_test_file = is_named_test_file(file_path);
    let is_conftest_file = file_path.file_name().and_then(|name| name.to_str()) == Some("conftest.incn");
    let test_module = ast.declarations.iter().find_map(|decl| match &decl.node {
        Declaration::TestModule(test_module) => Some(test_module),
        _ => None,
    });
    if is_named_test_file && test_module.is_some() {
        return Err("RFC 018 test files must not contain `module tests:`; put inline tests in production source files or use top-level test functions in test files".to_string());
    }

    let declarations: Vec<Spanned<Declaration>> = if is_named_test_file || is_conftest_file {
        ast.declarations.clone()
    } else if let Some(test_module) = test_module {
        test_module.body.clone()
    } else {
        Vec::new()
    };

    let mut import_aliases = crate::frontend::decorator_resolution::collect_import_aliases(&ast);
    if !is_named_test_file {
        let inline_program = program_for_decls(declarations.clone());
        import_aliases.extend(crate::frontend::decorator_resolution::collect_import_aliases(
            &inline_program,
        ));
    }
    let semantics =
        load_testing_marker_semantics().map_err(|e| format!("Failed to load std.testing marker semantics: {e}"))?;

    let mut tests = Vec::new();
    let mut fixtures = Vec::new();
    let mut default_marks = inherited_marks.to_vec();
    let mut known_markers = inherited_known_markers.to_vec();

    for decl in &declarations {
        if let Declaration::Const(konst) = &decl.node {
            if konst.name == "TEST_MARKS" {
                default_marks.extend(extract_string_list(&konst.value.node));
            } else if konst.name == "TEST_MARKERS" {
                known_markers.extend(extract_string_list(&konst.value.node));
            }
        }
    }

    let mut fixture_names: Vec<String> = Vec::new();
    fixture_names.extend(visible_fixture_names.iter().cloned());
    fixture_names.extend(BUILTIN_FIXTURES.iter().map(|name| (*name).to_string()));
    for decl in &declarations {
        if let Declaration::Function(func) = &decl.node
            && has_fixture_decorator(&func.decorators, &import_aliases, &semantics)
        {
            fixture_names.push(func.name.clone());
        }
    }

    for decl in &declarations {
        if let Declaration::Function(func) = &decl.node {
            if has_fixture_decorator(&func.decorators, &import_aliases, &semantics) {
                let (scope, autouse) = extract_fixture_args(&func.decorators, &import_aliases, &semantics);
                let dependencies = extract_fixture_dependencies(&func.params, &fixture_names);
                validate_fixture_parameters(&func.name, &func.params, &dependencies)?;
                let has_teardown = validate_yield_fixture(&func.name, &func.body, scope)?;

                fixtures.push(FixtureInfo {
                    name: func.name.clone(),
                    file_path: file_path.to_path_buf(),
                    scope,
                    autouse,
                    dependencies,
                    has_teardown,
                    is_async: func.is_async(),
                });
            } else if func.name.starts_with("test_")
                || has_test_decorator(&func.decorators, &import_aliases, &semantics)
            {
                let mut markers: Vec<TestMarker> = default_marks.iter().cloned().map(TestMarker::Mark).collect();
                markers.extend(extract_test_markers(
                    &func.decorators,
                    &import_aliases,
                    &semantics,
                    eval_context,
                )?);
                let timeout = markers.iter().rev().find_map(|marker| {
                    if let TestMarker::Timeout(duration) = marker {
                        Some(*duration)
                    } else {
                        None
                    }
                });
                let required_fixtures = extract_fixture_dependencies(&func.params, &fixture_names);
                validate_test_parameters(&func.name, &func.params, &required_fixtures, &markers)?;

                tests.push(TestInfo {
                    file_path: file_path.to_path_buf(),
                    function_name: func.name.clone(),
                    markers,
                    required_fixtures,
                    parameter_names: func.params.iter().map(|param| param.node.name.clone()).collect(),
                    timeout,
                    parametrize_call: None,
                });
            }
        }
    }

    default_marks.sort();
    default_marks.dedup();
    known_markers.sort();
    known_markers.dedup();
    Ok(DiscoveryResult {
        tests,
        fixtures,
        default_marks,
        known_markers,
    })
}

/// Reject fixture parameters that cannot be resolved as other fixtures.
fn validate_fixture_parameters(
    fixture_name: &str,
    params: &[crate::frontend::ast::Spanned<crate::frontend::ast::Param>],
    dependencies: &[String],
) -> Result<(), String> {
    for param in params {
        if !dependencies.contains(&param.node.name) {
            return Err(format!(
                "missing fixture `{}` required by fixture `{}`",
                param.node.name, fixture_name
            ));
        }
    }
    Ok(())
}

/// Reject test parameters that are neither fixtures nor parametrized arguments.
fn validate_test_parameters(
    test_name: &str,
    params: &[crate::frontend::ast::Spanned<crate::frontend::ast::Param>],
    required_fixtures: &[String],
    markers: &[TestMarker],
) -> Result<(), String> {
    let parametrized_names = parametrize_argument_names(markers);
    validate_parametrize_case_arity(test_name, markers)?;
    for param in params {
        if !required_fixtures.contains(&param.node.name) && !parametrized_names.contains(&param.node.name) {
            return Err(format!(
                "missing fixture `{}` required by test `{}`",
                param.node.name, test_name
            ));
        }
    }
    Ok(())
}

/// Reject parametrized cases whose value count does not match the declared argname count.
fn validate_parametrize_case_arity(test_name: &str, markers: &[TestMarker]) -> Result<(), String> {
    for marker in markers {
        let TestMarker::Parametrize(names, cases) = marker else {
            continue;
        };
        let argument_names = split_parametrize_argument_names(names);
        for case in cases {
            if case.rust_arguments.len() != argument_names.len() {
                return Err(format!(
                    "parametrize case `{}` for test `{}` expected {} value(s) for `{}`, got {}",
                    case.display_id,
                    test_name,
                    argument_names.len(),
                    names,
                    case.rust_arguments.len()
                ));
            }
        }
    }
    Ok(())
}

/// Return every argument name introduced by `@parametrize` markers on a test.
fn parametrize_argument_names(markers: &[TestMarker]) -> Vec<String> {
    markers
        .iter()
        .filter_map(|marker| {
            if let TestMarker::Parametrize(names, _) = marker {
                Some(names.as_str())
            } else {
                None
            }
        })
        .flat_map(split_parametrize_argument_names)
        .collect()
}

/// Split the comma-separated argname string used by `@parametrize`.
fn split_parametrize_argument_names(names: &str) -> Vec<String> {
    names
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

/// Check if a function has an explicit `@std.testing.test` decorator.
fn has_test_decorator(
    decorators: &[crate::frontend::ast::Spanned<crate::frontend::ast::Decorator>],
    aliases: &HashMap<String, Vec<String>>,
    semantics: &TestingMarkerSemantics,
) -> bool {
    decorators
        .iter()
        .any(|d| resolve_testing_marker_kind(&d.node, aliases, semantics) == Some(TestingMarkerKind::Test))
}

/// Check if a function has a fixture decorator.
fn has_fixture_decorator(
    decorators: &[crate::frontend::ast::Spanned<crate::frontend::ast::Decorator>],
    aliases: &HashMap<String, Vec<String>>,
    semantics: &TestingMarkerSemantics,
) -> bool {
    decorators
        .iter()
        .any(|d| resolve_testing_marker_kind(&d.node, aliases, semantics) == Some(TestingMarkerKind::Fixture))
}

/// Extract fixture arguments from a function's decorators.
fn extract_fixture_args(
    decorators: &[crate::frontend::ast::Spanned<crate::frontend::ast::Decorator>],
    aliases: &HashMap<String, Vec<String>>,
    semantics: &TestingMarkerSemantics,
) -> (FixtureScope, bool) {
    let mut scope = FixtureScope::default();
    let mut autouse = false;

    for dec in decorators {
        if resolve_testing_marker_kind(&dec.node, aliases, semantics) == Some(TestingMarkerKind::Fixture) {
            for arg in &dec.node.args {
                if let DecoratorArg::Named(name, value) = arg {
                    if name == &semantics.fixture_scope_arg {
                        if let DecoratorArgValue::Expr(expr) = value
                            && let Expr::Literal(Literal::String(s)) = &expr.node
                        {
                            scope = match s.as_str() {
                                value if value == semantics.fixture_scope_function.as_str() => FixtureScope::Function,
                                value if value == semantics.fixture_scope_module.as_str() => FixtureScope::Module,
                                value if value == semantics.fixture_scope_session.as_str() => FixtureScope::Session,
                                _ => FixtureScope::Function,
                            };
                        }
                    } else if name == &semantics.fixture_autouse_arg
                        && let DecoratorArgValue::Expr(expr) = value
                        && let Expr::Literal(Literal::Bool(b)) = &expr.node
                    {
                        autouse = *b;
                    }
                }
            }
        }
    }

    (scope, autouse)
}

/// Extract fixture dependencies from a function's parameters.
fn extract_fixture_dependencies(
    params: &[crate::frontend::ast::Spanned<crate::frontend::ast::Param>],
    fixture_names: &[String],
) -> Vec<String> {
    params
        .iter()
        .filter(|p| fixture_names.contains(&p.node.name))
        .map(|p| p.node.name.clone())
        .collect()
}

/// Check if a function has a yield statement.
fn function_has_yield(body: &[crate::frontend::ast::Spanned<crate::frontend::ast::Statement>]) -> bool {
    any_expr_in_body(body, |expr| matches!(expr, Expr::Yield(_)))
}

/// Validate the runner-local yield fixture subset and return whether teardown is present.
fn validate_yield_fixture(name: &str, body: &[Spanned<Statement>], scope: FixtureScope) -> Result<bool, String> {
    if !function_has_yield(body) {
        return Ok(false);
    }
    let _ = scope;

    let top_level_yields: Vec<usize> = body
        .iter()
        .enumerate()
        .filter_map(|(index, stmt)| {
            if matches!(&stmt.node, Statement::Expr(expr) if matches!(&expr.node, Expr::Yield(_))) {
                Some(index)
            } else {
                None
            }
        })
        .collect();
    let Some(yield_index) = top_level_yields.first().copied() else {
        return Err(format!(
            "fixture `{name}` uses unsupported `yield` teardown; `yield` must be a top-level expression statement"
        ));
    };
    if top_level_yields.len() != 1
        || body.iter().enumerate().any(|(index, stmt)| {
            index != yield_index && any_expr_in_body(std::slice::from_ref(stmt), |expr| matches!(expr, Expr::Yield(_)))
        })
    {
        return Err(format!(
            "fixture `{name}` uses unsupported `yield` teardown; exactly one top-level `yield` statement is supported"
        ));
    }

    Ok(true)
}

/// Get autouse fixtures for a given scope.
pub(crate) fn get_autouse_fixtures(fixtures: &HashMap<String, FixtureInfo>, scope: FixtureScope) -> Vec<String> {
    fixtures
        .values()
        .filter(|f| f.autouse && f.scope == scope)
        .map(|f| f.name.clone())
        .collect()
}

/// Extract test markers from the decorators.
fn extract_test_markers(
    decorators: &[crate::frontend::ast::Spanned<crate::frontend::ast::Decorator>],
    aliases: &HashMap<String, Vec<String>>,
    semantics: &TestingMarkerSemantics,
    eval_context: &CollectionEvalContext,
) -> Result<Vec<TestMarker>, String> {
    let mut markers = Vec::new();

    for dec in decorators {
        match resolve_testing_marker_kind(&dec.node, aliases, semantics) {
            Some(TestingMarkerKind::Skip) => {
                let reason = extract_string_arg(&dec.node.args).unwrap_or_default();
                markers.push(TestMarker::Skip(reason));
            }
            Some(TestingMarkerKind::SkipIf) => {
                if eval_conditional_marker(&dec.node.args, eval_context)? {
                    markers.push(TestMarker::Skip(extract_conditional_reason(&dec.node.args)));
                }
            }
            Some(TestingMarkerKind::XFail) => {
                let reason = extract_string_arg(&dec.node.args).unwrap_or_default();
                markers.push(TestMarker::XFail(reason));
            }
            Some(TestingMarkerKind::XFailIf) => {
                if eval_conditional_marker(&dec.node.args, eval_context)? {
                    markers.push(TestMarker::XFail(extract_conditional_reason(&dec.node.args)));
                }
            }
            Some(TestingMarkerKind::Slow) => {
                markers.push(TestMarker::Slow);
            }
            Some(TestingMarkerKind::Mark) => {
                if let Some(name) = extract_string_arg(&dec.node.args) {
                    markers.push(TestMarker::Mark(name));
                }
            }
            Some(TestingMarkerKind::Resource) => {
                if let Some(name) = extract_string_arg(&dec.node.args) {
                    markers.push(TestMarker::Resource(name));
                }
            }
            Some(TestingMarkerKind::Serial) => {
                markers.push(TestMarker::Serial);
            }
            Some(TestingMarkerKind::Timeout) => {
                if let Some(raw) = extract_string_arg(&dec.node.args)
                    && let Some(duration) = parse_duration_literal(&raw)
                {
                    markers.push(TestMarker::Timeout(duration));
                }
            }
            Some(TestingMarkerKind::Parametrize) => {
                let argnames = extract_string_arg(&dec.node.args).unwrap_or_default();
                let argvalues = extract_parametrize_argvalues(&dec.node.args);
                markers.push(TestMarker::Parametrize(argnames, argvalues));
            }
            Some(TestingMarkerKind::Test) => {
                markers.push(TestMarker::Test);
            }
            Some(TestingMarkerKind::Fixture) | None => {}
        }
    }

    Ok(markers)
}

#[derive(Debug, Clone, PartialEq)]
enum CollectionValue {
    Bool(bool),
    String(String),
}

/// Evaluate the first positional condition argument for `skipif` / `xfailif`.
fn eval_conditional_marker(
    args: &[crate::frontend::ast::DecoratorArg],
    context: &CollectionEvalContext,
) -> Result<bool, String> {
    let Some(DecoratorArg::Positional(expr)) = args.first() else {
        return Err("conditional testing marker requires a collection-time condition".to_string());
    };
    match eval_collection_expr(&expr.node, context)? {
        CollectionValue::Bool(value) => Ok(value),
        other => Err(format!(
            "conditional testing marker expression must evaluate to bool, got {:?}",
            other
        )),
    }
}

/// Extract the optional reason argument from `skipif` / `xfailif` decorators.
fn extract_conditional_reason(args: &[crate::frontend::ast::DecoratorArg]) -> String {
    for arg in args {
        if let DecoratorArg::Named(name, DecoratorArgValue::Expr(expr)) = arg
            && name == "reason"
            && let Expr::Literal(Literal::String(reason)) = &expr.node
        {
            return reason.clone();
        }
    }
    if let Some(DecoratorArg::Positional(expr)) = args.get(1)
        && let Expr::Literal(Literal::String(reason)) = &expr.node
    {
        return reason.clone();
    }
    String::new()
}

/// Evaluate the intentionally small RFC019 collection-time condition subset.
fn eval_collection_expr(expr: &Expr, context: &CollectionEvalContext) -> Result<CollectionValue, String> {
    match expr {
        Expr::Literal(Literal::Bool(value)) => Ok(CollectionValue::Bool(*value)),
        Expr::Literal(Literal::String(value)) => Ok(CollectionValue::String(value.clone())),
        Expr::Paren(inner) => eval_collection_expr(&inner.node, context),
        Expr::Unary(UnaryOp::Not, inner) => match eval_collection_expr(&inner.node, context)? {
            CollectionValue::Bool(value) => Ok(CollectionValue::Bool(!value)),
            other => Err(format!("`not` expects bool in testing condition, got {:?}", other)),
        },
        Expr::Unary(UnaryOp::Neg, _) => Err("numeric negation is not supported in testing conditions".to_string()),
        Expr::Binary(left, op, right) => eval_collection_binary(&left.node, *op, &right.node, context),
        Expr::Call(callee, type_args, args) if type_args.is_empty() => {
            let Expr::Ident(name) = &callee.node else {
                return Err(
                    "only platform() and feature(\"name\") calls are supported in testing conditions".to_string(),
                );
            };
            eval_collection_probe(name, args, context)
        }
        _ => Err("unsupported testing condition; use literals, bool operators, platform(), or feature()".to_string()),
    }
}

/// Evaluate a supported binary operator inside a collection-time marker condition.
fn eval_collection_binary(
    left: &Expr,
    op: BinaryOp,
    right: &Expr,
    context: &CollectionEvalContext,
) -> Result<CollectionValue, String> {
    match op {
        BinaryOp::And => {
            let left = expect_collection_bool(eval_collection_expr(left, context)?, "and")?;
            if !left {
                return Ok(CollectionValue::Bool(false));
            }
            Ok(CollectionValue::Bool(expect_collection_bool(
                eval_collection_expr(right, context)?,
                "and",
            )?))
        }
        BinaryOp::Or => {
            let left = expect_collection_bool(eval_collection_expr(left, context)?, "or")?;
            if left {
                return Ok(CollectionValue::Bool(true));
            }
            Ok(CollectionValue::Bool(expect_collection_bool(
                eval_collection_expr(right, context)?,
                "or",
            )?))
        }
        BinaryOp::Eq | BinaryOp::NotEq => {
            let left = eval_collection_expr(left, context)?;
            let right = eval_collection_expr(right, context)?;
            let equal = left == right;
            Ok(CollectionValue::Bool(if op == BinaryOp::Eq { equal } else { !equal }))
        }
        _ => Err(format!(
            "operator `{op}` is not supported in testing collection conditions"
        )),
    }
}

/// Require a collection-time expression value to be boolean.
fn expect_collection_bool(value: CollectionValue, operator: &str) -> Result<bool, String> {
    match value {
        CollectionValue::Bool(value) => Ok(value),
        other => Err(format!(
            "`{operator}` expects bool operands in testing condition, got {:?}",
            other
        )),
    }
}

/// Evaluate supported `std.testing` collection probes.
fn eval_collection_probe(
    name: &str,
    args: &[CallArg],
    context: &CollectionEvalContext,
) -> Result<CollectionValue, String> {
    match name {
        "platform" if args.is_empty() => Ok(CollectionValue::String(context.platform.clone())),
        "feature" => {
            let Some(CallArg::Positional(value)) = args.first() else {
                return Err("feature() requires one string argument".to_string());
            };
            if args.len() != 1 {
                return Err("feature() accepts exactly one string argument".to_string());
            }
            let Expr::Literal(Literal::String(name)) = &value.node else {
                return Err("feature() requires a string literal argument".to_string());
            };
            Ok(CollectionValue::Bool(context.features.contains(name)))
        }
        "platform" => Err("platform() accepts no arguments".to_string()),
        _ => Err("only platform() and feature(\"name\") calls are supported in testing conditions".to_string()),
    }
}

/// Extract a string argument from a decorator's arguments.
fn extract_string_arg(args: &[crate::frontend::ast::DecoratorArg]) -> Option<String> {
    if let Some(DecoratorArg::Positional(expr)) = args.first()
        && let Expr::Literal(Literal::String(s)) = &expr.node
    {
        return Some(s.clone());
    }
    None
}

/// Extract string literals from a list expression.
fn extract_string_list(expr: &Expr) -> Vec<String> {
    let Expr::List(items) = expr else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            if let ListEntry::Element(value) = item
                && let Expr::Literal(Literal::String(value)) = &value.node
            {
                return Some(value.clone());
            }
            None
        })
        .collect()
}

/// Parse RFC 019 duration strings such as `250ms`, `5s`, `2m`, and `1h`.
pub(crate) fn parse_duration_literal(raw: &str) -> Option<Duration> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    let split_at = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split_at);
    let amount = number.parse::<f64>().ok()?;
    if amount.is_sign_negative() {
        return None;
    }
    let seconds = match unit.trim() {
        "ms" => amount / 1000.0,
        "s" | "" => amount,
        "m" => amount * 60.0,
        "h" => amount * 60.0 * 60.0,
        _ => return None,
    };
    Some(Duration::from_secs_f64(seconds))
}

/// Extract parametrize argvalues from the second positional decorator argument.
///
/// The second argument is expected to be a list of tuples (or single values).  For each element the function produces a
/// [`ParametrizeCase`] carrying a display ID (dash-separated) and a Rust argument list (comma-separated, with string
/// literals wrapped in `.to_string()`).
fn extract_parametrize_argvalues(args: &[crate::frontend::ast::DecoratorArg]) -> Vec<ParametrizeCase> {
    let Some(DecoratorArg::Positional(list_expr)) = args.get(1) else {
        return Vec::new();
    };
    let Expr::List(items) = &list_expr.node else {
        return Vec::new();
    };

    let ids = extract_parametrize_ids(args);
    let mut case_index = 0usize;
    items
        .iter()
        .filter_map(|item| match item {
            crate::frontend::ast::ListEntry::Element(value) => {
                let explicit_id = ids.get(case_index).cloned();
                case_index += 1;
                Some(build_parametrize_case(value, explicit_id))
            }
            crate::frontend::ast::ListEntry::Spread(_) => None,
        })
        .collect()
}

/// Extract explicit case IDs from the `ids=[...]` named argument.
fn extract_parametrize_ids(args: &[crate::frontend::ast::DecoratorArg]) -> Vec<String> {
    args.iter()
        .find_map(|arg| {
            if let DecoratorArg::Named(name, DecoratorArgValue::Expr(expr)) = arg
                && name == "ids"
            {
                Some(extract_string_list(&expr.node))
            } else {
                None
            }
        })
        .unwrap_or_default()
}

/// Build a [`ParametrizeCase`] from a single parameter-set expression.
///
/// Tuples like `(1, 2, 3)` yield display ID `"1-2-3"` and Rust args `"1, 2, 3"`.
fn build_parametrize_case(expr: &Spanned<Expr>, explicit_id: Option<String>) -> ParametrizeCase {
    if let Some((value, markers, id)) = extract_case_call(expr) {
        let mut case = build_parametrize_case(value, id.or(explicit_id));
        case.markers = markers;
        return case;
    }

    match &expr.node {
        Expr::Tuple(elements) => {
            let derived_display_id = elements
                .iter()
                .map(|e| render_display(&e.node))
                .collect::<Vec<_>>()
                .join("-");
            let display_id = explicit_id.unwrap_or(derived_display_id);
            let rust_arguments = elements.iter().map(|e| render_rust_arg(&e.node)).collect();
            let parameters = elements.iter().map(|e| render_display(&e.node)).collect();
            ParametrizeCase {
                display_id,
                rust_arguments,
                parameters,
                markers: Vec::new(),
            }
        }
        other => {
            let rendered = render_display(other);
            let display_id = explicit_id.unwrap_or_else(|| rendered.clone());
            let rust_args = render_rust_arg(other);
            ParametrizeCase {
                display_id,
                rust_arguments: vec![rust_args],
                parameters: vec![rendered],
                markers: Vec::new(),
            }
        }
    }
}

/// Extract `param_case(value, marks=..., id=...)` metadata from one parameter-set expression.
fn extract_case_call(expr: &Spanned<Expr>) -> Option<(&Spanned<Expr>, Vec<TestMarker>, Option<String>)> {
    let args = match &expr.node {
        Expr::Call(callee, _, args) if matches!(&callee.node, Expr::Ident(name) if name == "param_case") => args,
        Expr::Constructor(name, args) if name == "param_case" => args,
        _ => return None,
    };
    let value = args.iter().find_map(|arg| {
        if let CallArg::Positional(value) = arg {
            Some(value)
        } else {
            None
        }
    })?;
    let mut markers = Vec::new();
    let mut id = None;
    for arg in args {
        if let CallArg::Named(name, value) = arg {
            if name == "id" {
                if let Expr::Literal(Literal::String(value)) = &value.node {
                    id = Some(value.clone());
                }
            } else if name == "marks" {
                markers.extend(extract_case_marks(&value.node));
            }
        }
    }
    Some((value, markers, id))
}

/// Extract per-case markers from a `param_case(..., marks=[...])` list.
fn extract_case_marks(expr: &Expr) -> Vec<TestMarker> {
    let Expr::List(items) = expr else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            ListEntry::Element(value) => marker_from_expr(&value.node),
            ListEntry::Spread(_) => None,
        })
        .collect()
}

/// Convert a per-case marker expression into runner marker metadata.
fn marker_from_expr(expr: &Expr) -> Option<TestMarker> {
    match expr {
        Expr::Literal(Literal::String(name)) => Some(TestMarker::Mark(name.clone())),
        Expr::Call(callee, _, args) => {
            let Expr::Ident(name) = &callee.node else {
                return None;
            };
            marker_from_call(name, args)
        }
        Expr::Constructor(name, args) => marker_from_call(name, args),
        Expr::Ident(name) if name == "slow" => Some(TestMarker::Slow),
        Expr::Ident(name) if name == "serial" => Some(TestMarker::Serial),
        _ => None,
    }
}

/// Convert a per-case marker call into runner marker metadata.
fn marker_from_call(name: &str, args: &[CallArg]) -> Option<TestMarker> {
    let string_arg = args.iter().find_map(|arg| {
        if let CallArg::Positional(value) = arg
            && let Expr::Literal(Literal::String(value)) = &value.node
        {
            Some(value.clone())
        } else {
            None
        }
    });
    match name {
        "skip" => Some(TestMarker::Skip(string_arg.unwrap_or_default())),
        "xfail" => Some(TestMarker::XFail(string_arg.unwrap_or_default())),
        "mark" => string_arg.map(TestMarker::Mark),
        "resource" => string_arg.map(TestMarker::Resource),
        "timeout" => string_arg.and_then(|raw| parse_duration_literal(&raw).map(TestMarker::Timeout)),
        _ => None,
    }
}

/// Render an expression as a short display string for test case IDs.
fn render_display(expr: &Expr) -> String {
    match expr {
        Expr::Literal(Literal::Int(il)) => il.value.to_string(),
        Expr::Literal(Literal::Float(f)) => f.value.to_string(),
        Expr::Literal(Literal::String(s)) => s.clone(),
        Expr::Literal(Literal::Bool(b)) => b.to_string(),
        Expr::Unary(crate::frontend::ast::UnaryOp::Neg, operand) => {
            format!("-{}", render_display(&operand.node))
        }
        Expr::Paren(inner) => render_display(&inner.node),
        _ => "?".to_string(),
    }
}

/// Render an expression as a Rust literal suitable for injection into generated code.
///
/// - Integers: `42`
/// - Floats: `1.5`
/// - Strings: `"hello".to_string()`
/// - Booleans: `true` / `false`
/// - Negative: `-42`
fn render_rust_arg(expr: &Expr) -> String {
    match expr {
        Expr::Literal(Literal::Int(il)) => il.value.to_string(),
        Expr::Literal(Literal::Float(f)) => f.value.to_string(),
        Expr::Literal(Literal::String(s)) => {
            format!("\"{}\".to_string()", s.replace('\\', "\\\\").replace('"', "\\\""))
        }
        Expr::Literal(Literal::Bool(b)) => b.to_string(),
        Expr::Unary(crate::frontend::ast::UnaryOp::Neg, operand) => {
            format!("-{}", render_rust_arg(&operand.node))
        }
        Expr::Paren(inner) => render_rust_arg(&inner.node),
        _ => "todo!(\"unsupported parametrize value\")".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Write Incan source to a temp file with a `test_` prefix so discovery recognises it.
    fn write_test_file(source: &str) -> Result<tempfile::NamedTempFile, Box<dyn std::error::Error>> {
        let mut file = tempfile::Builder::new().prefix("test_").suffix(".incn").tempfile()?;
        file.write_all(source.as_bytes())?;
        file.flush()?;
        Ok(file)
    }

    /// Write Incan source to a temp file that does not use a conventional test-file name.
    fn write_source_file(source: &str) -> Result<tempfile::NamedTempFile, Box<dyn std::error::Error>> {
        let mut file = tempfile::Builder::new().prefix("module_").suffix(".incn").tempfile()?;
        file.write_all(source.as_bytes())?;
        file.flush()?;
        Ok(file)
    }

    // ---- Basic test discovery ----

    #[test]
    fn discover_plain_test_functions() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import assert_eq

def helper() -> int:
    return 42

def test_one() -> None:
    assert_eq(helper(), 42)

def test_two() -> None:
    assert_eq(1, 1)
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.tests.len(), 2, "should discover two test functions");
        assert_eq!(result.fixtures.len(), 0, "no fixtures declared");

        let names: Vec<&str> = result.tests.iter().map(|t| t.function_name.as_str()).collect();
        assert!(names.contains(&"test_one"), "should find test_one");
        assert!(names.contains(&"test_two"), "should find test_two");

        for test in &result.tests {
            assert!(test.markers.is_empty(), "plain tests should have no markers");
            assert!(test.required_fixtures.is_empty(), "plain tests need no fixtures");
        }
        Ok(())
    }

    #[test]
    fn discover_ignores_non_test_functions() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
def helper() -> int:
    return 1

def setup() -> None:
    pass

def test_only_this() -> None:
    pass
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.tests.len(), 1, "only test_ prefixed functions are tests");
        assert_eq!(result.tests[0].function_name, "test_only_this");
        Ok(())
    }

    #[test]
    fn discover_test_files_includes_source_files_with_inline_module_tests() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let src_dir = dir.path().join("src");
        std::fs::create_dir_all(&src_dir)?;
        let inline_path = src_dir.join("math.incn");
        let helper_path = src_dir.join("helpers.incn");
        std::fs::write(
            &inline_path,
            r#"
def add(a: int, b: int) -> int:
    return a + b

module tests:
    from std.testing import assert_eq

    def test_add() -> None:
        assert_eq(add(2, 3), 5)
"#,
        )?;
        std::fs::write(
            &helper_path,
            r#"
def helper() -> int:
    return 42
"#,
        )?;

        let discovered = discover_test_files(dir.path());

        assert!(
            discovered.contains(&inline_path),
            "source file with inline `module tests:` should be discovered"
        );
        assert!(
            !discovered.contains(&helper_path),
            "ordinary source file without inline tests should not be discovered"
        );
        Ok(())
    }

    #[test]
    fn discover_inline_module_tests_and_fixtures() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import fixture

def test_not_discovered_from_production_scope() -> None:
    pass

def helper() -> int:
    return 42

module tests:
    from std.testing import fixture

    @fixture
    def inline_fixture() -> int:
        return helper()

    def test_inline(inline_fixture: int) -> None:
        pass
"#;
        let file = write_source_file(source)?;

        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(
            result.tests.len(),
            1,
            "only inline test-module functions are discovered"
        );
        assert_eq!(result.tests[0].function_name, "test_inline");
        assert_eq!(result.tests[0].required_fixtures, vec!["inline_fixture".to_string()]);
        assert_eq!(result.fixtures.len(), 1, "inline fixture is discovered");
        assert_eq!(result.fixtures[0].name, "inline_fixture");
        Ok(())
    }

    #[test]
    fn discover_test_file_rejects_module_tests() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
module tests:
    def test_inline() -> None:
        pass
"#;
        let file = write_test_file(source)?;
        let err = discover_tests_and_fixtures(file.path())
            .err()
            .ok_or("expected test file with inline module tests to fail discovery")?;

        assert!(
            err.contains("must not contain `module tests:`"),
            "expected RFC 018 inline-module diagnostic, got: {err}"
        );
        Ok(())
    }

    #[test]
    fn discover_explicit_test_decorator() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import assert_eq, test

@test
def computes_sum() -> None:
    assert_eq(1 + 1, 2)
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(
            result.tests.len(),
            1,
            "@test should discover functions without test_ prefix"
        );
        assert_eq!(result.tests[0].function_name, "computes_sum");
        assert!(result.tests[0].markers.contains(&TestMarker::Test));
        Ok(())
    }

    // ---- Marker discovery ----

    #[test]
    fn discover_skip_marker_with_reason() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import skip

@skip("not ready")
def test_skipped() -> None:
    pass
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.tests.len(), 1);
        assert_eq!(result.tests[0].markers.len(), 1);
        assert_eq!(result.tests[0].markers[0], TestMarker::Skip("not ready".to_string()));
        Ok(())
    }

    #[test]
    fn discover_skip_marker_without_reason() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import skip

@skip
def test_skipped() -> None:
    pass
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.tests.len(), 1);
        assert_eq!(result.tests[0].markers.len(), 1);
        assert_eq!(result.tests[0].markers[0], TestMarker::Skip(String::new()));
        Ok(())
    }

    #[test]
    fn discover_xfail_marker() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import xfail

@xfail("known bug #42")
def test_broken() -> None:
    pass
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.tests.len(), 1);
        assert_eq!(result.tests[0].markers.len(), 1);
        assert_eq!(
            result.tests[0].markers[0],
            TestMarker::XFail("known bug #42".to_string())
        );
        Ok(())
    }

    #[test]
    fn discover_slow_marker() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import slow

@slow
def test_integration() -> None:
    pass
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.tests.len(), 1);
        assert_eq!(result.tests[0].markers.len(), 1);
        assert_eq!(result.tests[0].markers[0], TestMarker::Slow);
        Ok(())
    }

    #[test]
    fn discover_multiple_markers_on_one_test() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import slow, xfail

@slow
@xfail("flaky")
def test_flaky_slow() -> None:
    pass
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.tests.len(), 1);
        assert_eq!(
            result.tests[0].markers.len(),
            2,
            "test should have both slow and xfail markers"
        );
        assert!(result.tests[0].markers.contains(&TestMarker::Slow));
        assert!(
            result.tests[0]
                .markers
                .contains(&TestMarker::XFail("flaky".to_string()))
        );
        Ok(())
    }

    // ---- Fixture discovery ----

    #[test]
    fn discover_fixture_default_scope() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import fixture

@fixture
def database() -> str:
    return "db"

def test_uses_db(database: str) -> None:
    pass
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.fixtures.len(), 1, "should discover one fixture");
        let fixture = &result.fixtures[0];
        assert_eq!(fixture.name, "database");
        assert_eq!(fixture.scope, FixtureScope::Function, "default scope is function");
        assert!(!fixture.autouse, "autouse is false by default");
        assert!(!fixture.has_teardown, "no yield means no teardown");

        assert_eq!(result.tests.len(), 1);
        assert_eq!(
            result.tests[0].required_fixtures,
            vec!["database".to_string()],
            "test should require the database fixture"
        );
        Ok(())
    }

    #[test]
    fn discover_fixture_module_scope() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import fixture

@fixture(scope="module")
def shared_client() -> str:
    return "client"
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.fixtures.len(), 1);
        assert_eq!(result.fixtures[0].scope, FixtureScope::Module);
        Ok(())
    }

    #[test]
    fn discover_fixture_session_scope() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import fixture

@fixture(scope="session")
def global_state() -> str:
    return "state"
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.fixtures.len(), 1);
        assert_eq!(result.fixtures[0].scope, FixtureScope::Session);
        Ok(())
    }

    #[test]
    fn discover_fixture_autouse() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import fixture

@fixture(autouse=true)
def setup_logging() -> None:
    pass
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.fixtures.len(), 1);
        assert!(result.fixtures[0].autouse, "autouse=true should be detected");
        Ok(())
    }

    #[test]
    fn discover_fixture_with_teardown() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import fixture

@fixture
def temp_file() -> str:
    path = "tmp.txt"
    yield path
    pass
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.fixtures.len(), 1);
        assert!(result.fixtures[0].has_teardown, "yield should be detected as teardown");
        Ok(())
    }

    #[test]
    fn discover_fixture_with_teardown_in_assignment_expression_errors() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import fixture

@fixture
def temp_file() -> str:
    path = (yield "tmp.txt")
    return path
"#;
        let file = write_test_file(source)?;
        let err = match discover_tests_and_fixtures(file.path()) {
            Ok(_) => return Err(std::io::Error::other("assignment-expression yield should error").into()),
            Err(err) => err,
        };

        assert!(
            err.contains("top-level expression statement"),
            "unexpected diagnostic: {err}"
        );
        Ok(())
    }

    #[test]
    fn discover_fixture_teardown_allows_setup_local_reference() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import fixture

@fixture
def temp_file() -> str:
    path = "tmp.txt"
    yield path
    print(path)
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.fixtures.len(), 1);
        assert!(result.fixtures[0].has_teardown);
        Ok(())
    }

    #[test]
    fn discover_fixture_teardown_allows_module_scope() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import fixture

@fixture(scope="module")
def shared() -> int:
    yield 1
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.fixtures.len(), 1);
        assert_eq!(result.fixtures[0].scope, FixtureScope::Module);
        assert!(result.fixtures[0].has_teardown);
        Ok(())
    }

    #[test]
    fn discover_fixture_dependency_chain() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import fixture

@fixture
def database() -> str:
    return "db"

@fixture
def populated_db(database: str) -> str:
    return "populated"

def test_query(populated_db: str) -> None:
    pass
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.fixtures.len(), 2);

        let populated = result.fixtures.iter().find(|f| f.name == "populated_db");
        assert!(populated.is_some(), "populated_db fixture should be discovered");
        assert_eq!(
            populated.map(|f| &f.dependencies),
            Some(&vec!["database".to_string()]),
            "populated_db should depend on database"
        );

        assert_eq!(result.tests.len(), 1);
        assert_eq!(
            result.tests[0].required_fixtures,
            vec!["populated_db".to_string()],
            "test should require populated_db"
        );
        Ok(())
    }

    // ---- Parametrize discovery ----

    #[test]
    fn discover_parametrize_marker_is_detected() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import parametrize, assert_eq

@parametrize("x, expected", [(1, 2), (2, 4)])
def test_double(x: int, expected: int) -> None:
    assert_eq(x * 2, expected)
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.tests.len(), 1, "parametrized test should be discovered");
        assert_eq!(result.tests[0].function_name, "test_double");

        let has_parametrize = result.tests[0]
            .markers
            .iter()
            .any(|m| matches!(m, TestMarker::Parametrize(_, _)));
        assert!(has_parametrize, "parametrize marker should be present");
        Ok(())
    }

    #[test]
    fn discover_parametrize_extracts_argnames() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import parametrize, assert_eq

@parametrize("x, y, expected", [(1, 2, 3), (0, 0, 0)])
def test_add(x: int, y: int, expected: int) -> None:
    assert_eq(x + y, expected)
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        assert_eq!(result.tests.len(), 1);
        let marker = result.tests[0]
            .markers
            .iter()
            .find(|m| matches!(m, TestMarker::Parametrize(_, _)));
        assert!(marker.is_some(), "parametrize marker should be present");

        if let Some(TestMarker::Parametrize(argnames, cases)) = marker {
            assert_eq!(
                argnames, "x, y, expected",
                "parametrize should extract the argnames string from the decorator"
            );
            assert_eq!(cases.len(), 2, "should have two parameter sets");
            assert_eq!(cases[0].display_id, "1-2-3");
            assert_eq!(cases[0].rust_arguments.join(", "), "1, 2, 3");
            assert_eq!(cases[1].display_id, "0-0-0");
            assert_eq!(cases[1].rust_arguments.join(", "), "0, 0, 0");
        }
        Ok(())
    }

    #[test]
    fn discover_parametrize_with_negative_values() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import parametrize, assert_eq

@parametrize("a, b, expected", [(1, 2, 3), (-1, 1, 0), (100, 200, 300)])
def test_add(a: int, b: int, expected: int) -> None:
    assert_eq(a + b, expected)
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        let marker = result.tests[0]
            .markers
            .iter()
            .find(|m| matches!(m, TestMarker::Parametrize(_, _)));

        if let Some(TestMarker::Parametrize(_, cases)) = marker {
            assert_eq!(cases.len(), 3, "should have three parameter sets");
            assert_eq!(cases[0].display_id, "1-2-3");
            assert_eq!(cases[0].rust_arguments.join(", "), "1, 2, 3");
            assert_eq!(cases[1].display_id, "-1-1-0");
            assert_eq!(cases[1].rust_arguments.join(", "), "-1, 1, 0");
            assert_eq!(cases[2].display_id, "100-200-300");
            assert_eq!(cases[2].rust_arguments.join(", "), "100, 200, 300");
        } else {
            return Err("expected parametrize marker".into());
        }
        Ok(())
    }

    #[test]
    fn discover_parametrize_with_string_values() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.testing import parametrize, assert_eq

@parametrize("input, expected", [("hello", 5), ("", 0)])
def test_len(input: str, expected: int) -> None:
    assert_eq(len(input), expected)
"#;
        let file = write_test_file(source)?;
        let result = discover_tests_and_fixtures(file.path())?;

        let marker = result.tests[0]
            .markers
            .iter()
            .find(|m| matches!(m, TestMarker::Parametrize(_, _)));

        if let Some(TestMarker::Parametrize(argnames, cases)) = marker {
            assert_eq!(argnames, "input, expected");
            assert_eq!(cases.len(), 2);
            assert_eq!(cases[0].display_id, "hello-5");
            assert_eq!(cases[0].rust_arguments.join(", "), "\"hello\".to_string(), 5");
            assert_eq!(cases[1].display_id, "-0");
            assert_eq!(cases[1].rust_arguments.join(", "), "\"\".to_string(), 0");
        } else {
            return Err("expected parametrize marker".into());
        }
        Ok(())
    }

    // ---- Test file naming patterns ----

    #[test]
    fn discover_test_file_patterns() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;

        // Should be discovered
        std::fs::write(dir.path().join("test_math.incn"), "def test_add() -> None:\n    pass\n")?;
        std::fs::write(
            dir.path().join("math_test.incn"),
            "def test_subtract() -> None:\n    pass\n",
        )?;

        // Should NOT be discovered
        std::fs::write(dir.path().join("helper.incn"), "def helper() -> None:\n    pass\n")?;
        std::fs::write(dir.path().join("test_helper.py"), "# not incan")?;

        let files = discover_test_files(dir.path());
        let names: Vec<String> = files
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
            .collect();

        assert_eq!(files.len(), 2, "should find exactly 2 test files");
        assert!(names.contains(&"test_math.incn".to_string()));
        assert!(names.contains(&"math_test.incn".to_string()));
        Ok(())
    }

    #[test]
    fn discover_single_file_input_returns_only_that_file() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let first = dir.path().join("test_alpha.incn");
        let second = dir.path().join("test_beta.incn");
        std::fs::write(&first, "def test_alpha() -> None:\n    pass\n")?;
        std::fs::write(&second, "def test_beta() -> None:\n    pass\n")?;

        let files = discover_test_files(&first);
        assert_eq!(files.len(), 1, "single-file input should not recurse the directory");
        assert_eq!(files[0], first);
        Ok(())
    }
}
