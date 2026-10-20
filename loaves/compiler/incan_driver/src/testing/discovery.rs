use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::session::CompilationSession;
use incan_frontend::ast::{
    BinaryOp, CallArg, Declaration, DecoratorArg, DecoratorArgValue, Expr, ListEntry, Literal, Program, Spanned,
    Statement, UnaryOp,
};
use incan_frontend::ast_walk::any_expr_in_body;
use incan_frontend::testing_markers::{TestingMarkerKind, TestingMarkerSemantics, resolve_testing_marker_kind};

use super::types::{DiscoveryResult, FixtureInfo, FixtureScope, ParametrizeCase, TestInfo, TestMarker};

/// Return whether `path` uses the conventional standalone test-file naming scheme.
fn is_named_test_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    (name.starts_with("test_") || name.ends_with("_test.incn")) && name.ends_with(".incn")
}

/// Parse one source file for collection-time discovery using the same dependency-provided vocabulary surfaces that
/// ordinary source compilation receives.
fn parse_collection_program(path: &Path, source: &str, session: &CompilationSession) -> Result<Program, String> {
    session
        .parse_source(path, source, false)
        .map_err(|e| format!("Parser error: {:?}", e))
}

/// Build a lightweight [`Program`] wrapper around a declaration slice so existing import-alias collection stays shared.
fn program_for_decls(declarations: Vec<Spanned<Declaration>>) -> Program {
    Program {
        declarations,
        source_path: None,
        rust_module_path: None,
        warnings: Vec::new(),
    }
}

/// Return whether declarations use any decorator supplied by `std.testing`.
fn declarations_use_testing_markers(
    declarations: &[Spanned<Declaration>],
    aliases: &HashMap<String, Vec<String>>,
) -> bool {
    declarations.iter().any(|declaration| match &declaration.node {
        Declaration::Function(function) => function.decorators.iter().any(|decorator| {
            let resolved = incan_frontend::decorator_resolution::resolve_decorator_path(&decorator.node, aliases);
            resolved.len() == 3 && resolved[0] == "std" && resolved[1] == "testing"
        }),
        Declaration::TestModule(test_module) => declarations_use_testing_markers(&test_module.body, aliases),
        _ => false,
    })
}

const BUILTIN_FIXTURES: &[&str] = &["tmp_path", "tmp_workdir", "env"];

/// Collection-time inputs available to conditional test markers.
#[derive(Debug, Clone)]
pub struct CollectionEvalContext {
    platform: String,
    features: BTreeSet<String>,
}

impl CollectionEvalContext {
    /// Create collection-time evaluation inputs from CLI-provided testing features.
    pub fn new(features: BTreeSet<String>) -> Self {
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

/// Discover test declarations using inherited conftest fixtures and marker registries.
pub fn discover_tests_and_fixtures_with_context(
    file_path: &Path,
    visible_fixture_names: &[String],
    inherited_marks: &[String],
    inherited_known_markers: &[String],
    eval_context: &CollectionEvalContext,
    compilation_session: &CompilationSession,
) -> Result<DiscoveryResult, String> {
    let source = fs::read_to_string(file_path).map_err(|e| format!("Failed to read file: {}", e))?;
    let ast = parse_collection_program(file_path, &source, compilation_session)?;

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

    let mut import_aliases = incan_frontend::decorator_resolution::collect_import_aliases(&ast);
    if !is_named_test_file {
        let inline_program = program_for_decls(declarations.clone());
        import_aliases.extend(incan_frontend::decorator_resolution::collect_import_aliases(
            &inline_program,
        ));
    }
    let semantics = if declarations_use_testing_markers(&declarations, &import_aliases) {
        compilation_session
            .require_testing_marker_semantics()
            .map_err(|error| format!("Failed to load std.testing marker semantics: {}", error.message))?
    } else {
        TestingMarkerSemantics::default()
    };

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
                    is_async: func.is_async(),
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
    params: &[incan_frontend::ast::Spanned<incan_frontend::ast::Param>],
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
    params: &[incan_frontend::ast::Spanned<incan_frontend::ast::Param>],
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
    decorators: &[incan_frontend::ast::Spanned<incan_frontend::ast::Decorator>],
    aliases: &HashMap<String, Vec<String>>,
    semantics: &TestingMarkerSemantics,
) -> bool {
    decorators
        .iter()
        .any(|d| resolve_testing_marker_kind(&d.node, aliases, semantics) == Some(TestingMarkerKind::Test))
}

/// Check if a function has a fixture decorator.
fn has_fixture_decorator(
    decorators: &[incan_frontend::ast::Spanned<incan_frontend::ast::Decorator>],
    aliases: &HashMap<String, Vec<String>>,
    semantics: &TestingMarkerSemantics,
) -> bool {
    decorators
        .iter()
        .any(|d| resolve_testing_marker_kind(&d.node, aliases, semantics) == Some(TestingMarkerKind::Fixture))
}

/// Extract fixture arguments from a function's decorators.
fn extract_fixture_args(
    decorators: &[incan_frontend::ast::Spanned<incan_frontend::ast::Decorator>],
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
    params: &[incan_frontend::ast::Spanned<incan_frontend::ast::Param>],
    fixture_names: &[String],
) -> Vec<String> {
    params
        .iter()
        .filter(|p| fixture_names.contains(&p.node.name))
        .map(|p| p.node.name.clone())
        .collect()
}

/// Check if a function has a yield statement.
fn function_has_yield(body: &[incan_frontend::ast::Spanned<incan_frontend::ast::Statement>]) -> bool {
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
pub fn get_autouse_fixtures(fixtures: &HashMap<String, FixtureInfo>, scope: FixtureScope) -> Vec<String> {
    fixtures
        .values()
        .filter(|f| f.autouse && f.scope == scope)
        .map(|f| f.name.clone())
        .collect()
}

/// Extract test markers from the decorators.
fn extract_test_markers(
    decorators: &[incan_frontend::ast::Spanned<incan_frontend::ast::Decorator>],
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
    args: &[incan_frontend::ast::DecoratorArg],
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
fn extract_conditional_reason(args: &[incan_frontend::ast::DecoratorArg]) -> String {
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
fn extract_string_arg(args: &[incan_frontend::ast::DecoratorArg]) -> Option<String> {
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
pub fn parse_duration_literal(raw: &str) -> Option<Duration> {
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
fn extract_parametrize_argvalues(args: &[incan_frontend::ast::DecoratorArg]) -> Vec<ParametrizeCase> {
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
            incan_frontend::ast::ListEntry::Element(value) => {
                let explicit_id = ids.get(case_index).cloned();
                case_index += 1;
                Some(build_parametrize_case(value, explicit_id))
            }
            incan_frontend::ast::ListEntry::Spread(_) => None,
        })
        .collect()
}

/// Extract explicit case IDs from the `ids=[...]` named argument.
fn extract_parametrize_ids(args: &[incan_frontend::ast::DecoratorArg]) -> Vec<String> {
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
            if name.node == "id" {
                if let Expr::Literal(Literal::String(value)) = &value.node {
                    id = Some(value.clone());
                }
            } else if name.node == "marks" {
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
        Expr::Unary(incan_frontend::ast::UnaryOp::Neg, operand) => {
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
        Expr::Unary(incan_frontend::ast::UnaryOp::Neg, operand) => {
            format!("-{}", render_rust_arg(&operand.node))
        }
        Expr::Paren(inner) => render_rust_arg(&inner.node),
        _ => "todo!(\"unsupported parametrize value\")".to_string(),
    }
}
