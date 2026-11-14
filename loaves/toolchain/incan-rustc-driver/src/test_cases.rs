//! Native entries for tests that take fixtures or `@parametrize` arguments.
//!
//! The native harness registers zero-argument functions, so a test that takes arguments runs through one generated
//! entry per case. Each entry sets up the test's fixtures by parameter name, passes the case's values and calls the
//! test, as 0.5.1 did: a fixture that depends on fixtures receives them as arguments, an autouse fixture runs after
//! the requested ones, a fixture's setup runs again for every parameter that names it, and `tmp_path`, `tmp_workdir`
//! and `env` are provided by the runner. A fixture that `yield`s its value becomes one stage of the entry: the
//! statements before `yield` set it up, the rest of the case runs where it yielded, and the statements after it tear
//! it down, also after a failure (`crate::test_entries`). An async test, or a test whose fixtures are async, gets an
//! async entry that awaits them.
//!
//! The entries are Incan source appended to the test file and checked with it, so diagnostics, lowering and the
//! harness see ordinary functions. The file's `conftest.incn` files follow its own text (`crate::test_conftest`). The
//! file's text is otherwise unchanged except that a `yield` fixture's own declaration is blanked line for line: its
//! stages replace it, and every later line keeps its number.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::test_conftest::{Conftest, appended};
use crate::test_entries::{EntryWriter, PREFIX, Stage};
use incan_driver::session::CompilationSession;
use incan_driver::testing::types::{DiscoveryResult, FixtureInfo, ParametrizeCase, TestInfo, TestMarker};
use incan_frontend::ast::{CallArg, Declaration, DecoratorArg, Expr, FunctionDecl, ListEntry, Spanned};
use incan_frontend::decorator_resolution::collect_import_aliases;
use incan_frontend::testing_markers::{TestingMarkerKind, resolve_testing_marker_kind};

/// The crate path 0.5.1's documentation imports `TestEnv` from, and the standard testing crate that provides it now.
const LEGACY_TESTING_CRATE: (&str, &str) = ("rust::incan_stdlib::testing", "rust::incan_std_testing");

/// One case the harness runs: the name reports give it, the function it registers and the case's own markers.
pub(crate) struct NativeCase {
    /// Position of the case's test in the discovery result.
    pub(crate) test: usize,
    /// `test_add[1-2-3]` for a parametrized case, otherwise the test's name.
    pub(crate) name: String,
    /// The zero-argument function the harness registers.
    pub(crate) entry: String,
    /// Each parametrized argument's display value, in parameter-set order.
    pub(crate) parameters: Vec<String>,
    /// Markers a `param_case(..., marks=[...])` adds to this case alone.
    pub(crate) markers: Vec<TestMarker>,
}

/// Every case of a test unit, and the test file's source with its generated entries when any case needs one.
pub(crate) struct NativeCases {
    pub(crate) cases: Vec<NativeCase>,
    pub(crate) source: Option<String>,
}

/// One parameter set of a parametrized test: its case id, each argument's source text by parameter name, its display
/// values and its own markers. A test without `@parametrize` has one row with no id.
#[derive(Clone, Default)]
struct Row {
    ids: Vec<String>,
    values: HashMap<String, String>,
    displays: Vec<String>,
    markers: Vec<TestMarker>,
}

/// Plan the native cases of a test unit.
///
/// 1. A unit without fixtures, autouse conftest fixtures or test parameters keeps one case per test.
/// 2. Otherwise check the test file with its conftests appended; inline `module tests:` keeps refusing.
/// 3. Expand each test into its parameter rows and write one entry per row.
/// 4. Return the checked text: `yield` fixtures blanked, then the entries, their helpers and their imports.
pub(crate) fn plan(
    source_path: &Path,
    session: &CompilationSession,
    discovery: &DiscoveryResult,
    conftests: &[Conftest],
) -> Result<NativeCases, String> {
    // 1. Plain tests register themselves.
    let shared_autouse = conftests
        .iter()
        .any(|conftest| conftest.discovery.fixtures.iter().any(|fixture| fixture.autouse));
    let needs_entries = !discovery.fixtures.is_empty()
        || shared_autouse
        || discovery.tests.iter().any(|test| !test.parameter_names.is_empty());
    let plain = || NativeCases {
        cases: discovery
            .tests
            .iter()
            .enumerate()
            .map(|(index, test)| plain_case(index, test))
            .collect(),
        source: None,
    };
    if !needs_entries {
        return Ok(plain());
    }

    // 2. The checked text.
    if !is_named_test_file(source_path) {
        return Err(
            "unsupported test unit fixtures or parameters in an inline test module on the native harness".into(),
        );
    }
    let own = std::fs::read_to_string(source_path)
        .map_err(|error| format!("cannot read test source {}: {error}", source_path.display()))?;
    let own_program = parse(session, source_path, &own)?;
    let own_imports: HashSet<String> = collect_import_aliases(&own_program).into_keys().collect();
    let (shared_text, shared) = appended(conftests, &discovery.fixtures, own_imports, session)?;
    let combined = format!("{own}{shared_text}").replace(LEGACY_TESTING_CRATE.0, LEGACY_TESTING_CRATE.1);
    let program = parse(session, source_path, &combined)?;
    let aliases = collect_import_aliases(&program);
    let declarations = program.declarations.as_slice();
    let functions: HashMap<String, &FunctionDecl> = declarations
        .iter()
        .filter_map(|declaration| match &declaration.node {
            Declaration::Function(function) => Some((function.name.clone(), function)),
            _ => None,
        })
        .collect();
    let fixtures: Vec<&FixtureInfo> = shared.iter().copied().chain(discovery.fixtures.iter()).collect();
    let writer = EntryWriter::new(
        &combined,
        fixtures
            .iter()
            .map(|fixture| (fixture.name.clone(), *fixture))
            .collect(),
        functions,
    );
    let semantics = session
        .require_testing_marker_semantics()
        .map_err(|error| error.message)?;

    // 3. One entry per parameter row.
    let mut cases = Vec::new();
    let mut generated = String::new();
    for (index, test) in discovery.tests.iter().enumerate() {
        let autouse = autouse_fixtures(&fixtures, test);
        if test.parameter_names.is_empty() && autouse.is_empty() {
            cases.push(plain_case(index, test));
            continue;
        }
        let function = writer.function(&test.function_name)?;
        let parametrized = parametrize_decorators(function, &aliases, &semantics);
        let text = |node: &Spanned<Expr>| writer.text(node.span.start, node.span.end).to_string();
        for row in rows(test, &parametrized, &text)? {
            let entry = format!("{PREFIX}_{}", cases.len());
            generated.push_str(&write_entry(&writer, &entry, test, function, &row, &autouse)?);
            let name = if row.ids.is_empty() {
                test.function_name.clone()
            } else {
                format!("{}[{}]", test.function_name, row.ids.join("-"))
            };
            cases.push(NativeCase {
                test: index,
                name,
                entry,
                parameters: row.displays,
                markers: row.markers,
            });
        }
    }

    // 4. The checked text.
    let mut checked = writer.blank_yield_fixtures(declarations);
    if !checked.ends_with('\n') {
        checked.push('\n');
    }
    checked.push_str(&writer.finish(&generated));
    Ok(NativeCases {
        cases,
        source: Some(checked),
    })
}

/// Parse a test file's text with the session's vocabulary.
fn parse(session: &CompilationSession, path: &Path, text: &str) -> Result<incan_frontend::ast::Program, String> {
    session
        .parse_source(path, text, false)
        .map_err(|errors| format!("cannot parse test source {}: {errors:?}", path.display()))
}

/// Write one case's entry: a stage per `yield` fixture, outermost first, then the stage that calls the test. Stage
/// `k` receives the values of the `yield` fixtures before it and sets up its fixture's other parameters; its body is
/// the fixture's own statements, with the next stage called where the fixture yields.
fn write_entry(
    writer: &EntryWriter<'_>,
    entry: &str,
    test: &TestInfo,
    function: &FunctionDecl,
    row: &Row,
    autouse: &[&str],
) -> Result<String, String> {
    let returns = return_type(writer, function);
    let returns_value = returns != "None";
    let mut roots: Vec<&str> = test
        .parameter_names
        .iter()
        .filter(|name| !row.values.contains_key(name.as_str()))
        .map(String::as_str)
        .collect();
    roots.extend(autouse.iter().copied());
    let stages = writer.stages(&roots)?;
    let asynchronous = writer.awaits(test, &roots);
    let stage_name = |position: usize| {
        if position == 0 {
            entry.to_string()
        } else {
            format!("{entry}_stage_{position}")
        }
    };
    let stage_at = |position: usize, returns: String| Stage {
        name: stage_name(position),
        next: stage_name(position + 1),
        returns,
        returns_value,
        asynchronous,
    };
    let mut written = String::new();
    let mut staged: Vec<&str> = Vec::new();
    let mut bound: Vec<String> = Vec::new();
    for (position, fixture) in stages.iter().enumerate() {
        written.push_str(&writer.yield_stage(&stage_at(position, returns.clone()), fixture, &staged, &bound)?);
        staged.push(fixture.name.as_str());
        bound.push(format!(
            "{}: {}",
            fixture.name,
            return_type(writer, writer.function(&fixture.name)?)
        ));
    }
    let last = stage_at(stages.len(), returns);
    written.push_str(&writer.call_stage(&last, test, function, &row.values, autouse, &staged, &bound)?);
    Ok(written)
}

/// A function's written return type, `None` when it declares none.
fn return_type(writer: &EntryWriter<'_>, function: &FunctionDecl) -> String {
    let written = writer
        .text(function.return_type.span.start, function.return_type.span.end)
        .trim();
    if matches!(function.return_type.node, incan_frontend::ast::Type::Unit) || written.is_empty() {
        "None".to_string()
    } else {
        written.to_string()
    }
}

/// A test that needs no entry registers under its own name.
fn plain_case(index: usize, test: &TestInfo) -> NativeCase {
    NativeCase {
        test: index,
        name: test.function_name.clone(),
        entry: test.function_name.clone(),
        parameters: Vec::new(),
        markers: Vec::new(),
    }
}

/// Whether `path` is a standalone test file, whose tests and fixtures are top-level declarations.
fn is_named_test_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
    name.ends_with(".incn") && (name.starts_with("test_") || name.ends_with("_test.incn"))
}

/// The autouse fixtures a test does not already request: the shared ones, then the file's, in declaration order.
fn autouse_fixtures<'a>(fixtures: &[&'a FixtureInfo], test: &TestInfo) -> Vec<&'a str> {
    fixtures
        .iter()
        .copied()
        .filter(|fixture| fixture.autouse && !test.parameter_names.contains(&fixture.name))
        .map(|fixture| fixture.name.as_str())
        .collect()
}

/// The test's `@parametrize` decorators in source order, each as its argument names and its argument-value list.
fn parametrize_decorators<'a>(
    function: &'a FunctionDecl,
    aliases: &HashMap<String, Vec<String>>,
    semantics: &incan_frontend::testing_markers::TestingMarkerSemantics,
) -> Vec<(Vec<String>, Vec<&'a Spanned<Expr>>)> {
    let mut found = Vec::new();
    for decorator in &function.decorators {
        if resolve_testing_marker_kind(&decorator.node, aliases, semantics) != Some(TestingMarkerKind::Parametrize) {
            continue;
        }
        let names = match decorator.node.args.first() {
            Some(DecoratorArg::Positional(expr)) => match &expr.node {
                Expr::Literal(incan_frontend::ast::Literal::String(names)) => names
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        let values = match decorator.node.args.get(1) {
            Some(DecoratorArg::Positional(list)) => match &list.node {
                Expr::List(items) => items
                    .iter()
                    .filter_map(|item| match item {
                        ListEntry::Element(value) => Some(value),
                        ListEntry::Spread(_) => None,
                    })
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        found.push((names, values));
    }
    found
}

/// Expand a test's `@parametrize` decorators into parameter rows: the cartesian product in decorator order, the first
/// decorator varying slowest, as 0.5.1 orders `test_stacked[1-a]`, `test_stacked[1-b]`, `test_stacked[2-a]`.
fn rows(
    test: &TestInfo,
    decorators: &[(Vec<String>, Vec<&Spanned<Expr>>)],
    text: &dyn Fn(&Spanned<Expr>) -> String,
) -> Result<Vec<Row>, String> {
    let collected: Vec<&Vec<ParametrizeCase>> = test
        .markers
        .iter()
        .filter_map(|marker| match marker {
            TestMarker::Parametrize(_, cases) => Some(cases),
            _ => None,
        })
        .collect();
    if collected.len() != decorators.len() {
        return Err(format!(
            "unsupported test contract for `{}`: a `@parametrize` decorator the native harness cannot read",
            test.function_name
        ));
    }
    let mut rows = vec![Row::default()];
    for ((names, values), cases) in decorators.iter().zip(collected) {
        if values.len() != cases.len() {
            return Err(format!(
                "unsupported test contract for `{}`: `@parametrize` values must be listed one by one",
                test.function_name
            ));
        }
        let mut expanded = Vec::new();
        for row in &rows {
            for (value, case) in values.iter().zip(cases) {
                let arguments = case_arguments(value);
                if arguments.len() != names.len() {
                    return Err(format!(
                        "parametrize case `{}` for test `{}` expected {} value(s), got {}",
                        case.display_id,
                        test.function_name,
                        names.len(),
                        arguments.len()
                    ));
                }
                let mut next = row.clone();
                next.ids.push(case.display_id.clone());
                for (name, argument) in names.iter().zip(arguments) {
                    next.values.insert(name.clone(), text(argument));
                }
                next.displays.extend(case.parameters.iter().cloned());
                next.markers.extend(case.markers.iter().cloned());
                expanded.push(next);
            }
        }
        rows = expanded;
    }
    Ok(rows)
}

/// The argument expressions of one parameter set: a tuple's elements, a single value, or `param_case`'s value.
fn case_arguments(value: &Spanned<Expr>) -> Vec<&Spanned<Expr>> {
    let case_value = match &value.node {
        Expr::Call(callee, _, args) if matches!(&callee.node, Expr::Ident(name) if name == "param_case") => {
            first_positional(args)
        }
        Expr::Constructor(name, args) if name == "param_case" => first_positional(args),
        _ => None,
    };
    let value = case_value.unwrap_or(value);
    match &value.node {
        Expr::Tuple(elements) => elements.iter().collect(),
        _ => vec![value],
    }
}

/// The first positional argument of a call.
fn first_positional(args: &[CallArg]) -> Option<&Spanned<Expr>> {
    args.iter().find_map(|arg| match arg {
        CallArg::Positional(value) => Some(value),
        _ => None,
    })
}
