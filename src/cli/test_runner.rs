//! Test runner implementation (pytest-style)
//!
//! ## TestReporter Trait
//!
//! The test runner uses a `TestReporter` trait to separate reporting from
//! execution. This allows for custom output formats (JSON, TAP, etc.) by
//! implementing the trait.
//!
//! ## I/O Boundaries
//!
//! Test discovery, harness generation, and execution are abstracted via traits
//! in `test_interfaces.rs` to allow for:
//! - Dry-run modes
//! - Custom execution strategies
//! - Mocking/testing of test runner logic
//!
//! Default implementations preserve current behavior.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::backend::{IrCodegen, ProjectGenerator};
use crate::frontend::{lexer, parser};
use incan_core::lang::decorators::{self, DecoratorId};

#[allow(unused_imports)]
use super::test_interfaces::{
    DefaultHarnessGenerator, DefaultTestDiscovery, DefaultTestExecutor, HarnessGenerator, HarnessInput, HarnessOutput,
    TestDiscovery, TestExecutor,
};
use super::{CliError, CliResult, ExitCode};

// ============================================================================
// Test Reporter Trait
// ============================================================================

/// Trait for reporting test execution results.
///
/// Implement this trait to customize test output format (JSON, TAP, etc.)
pub trait TestReporter {
    /// Called when test discovery begins
    fn on_discovery_start(&mut self, _path: &str) {}

    /// Called when a test file is discovered
    fn on_file_discovered(&mut self, _path: &Path) {}

    /// Called when test collection is complete
    fn on_collection_complete(&mut self, test_count: usize);

    /// Called when a test run begins
    fn on_test_start(&mut self, test: &TestInfo);

    /// Called when a test completes
    fn on_test_complete(&mut self, test: &TestInfo, result: &TestResult);

    /// Called when all tests have completed
    fn on_run_complete(&mut self, summary: &TestSummary);
}

/// Summary of test run
pub struct TestSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub xfailed: usize,
    pub duration: Duration,
}

/// Default console reporter (pytest-style)
#[derive(Default)]
pub struct ConsoleReporter {
    pub verbose: bool,
    pub use_color: bool,
}

impl ConsoleReporter {
    pub fn new(verbose: bool, use_color: bool) -> Self {
        Self { verbose, use_color }
    }
}

fn style<T: fmt::Display>(text: T, code: &str, use_color: bool) -> String {
    if use_color {
        format!("\x1b[{}m{}\x1b[0m", code, text)
    } else {
        text.to_string()
    }
}

impl TestReporter for ConsoleReporter {
    fn on_collection_complete(&mut self, test_count: usize) {
        if test_count == 0 {
            eprintln!("No tests collected");
        }
    }

    fn on_test_start(&mut self, test: &TestInfo) {
        if self.verbose {
            eprint!("{} ... ", test.function_name);
        }
    }

    fn on_test_complete(&mut self, test: &TestInfo, result: &TestResult) {
        let use_color = self.use_color;
        let status = match result {
            TestResult::Passed(d) => {
                if self.verbose {
                    format!("{} ({:.0}ms)", style("PASSED", "32", use_color), d.as_millis())
                } else {
                    style(".", "32", use_color)
                }
            }
            TestResult::Failed(d, _) => {
                if self.verbose {
                    format!("{} ({:.0}ms)", style("FAILED", "31", use_color), d.as_millis())
                } else {
                    style("F", "31", use_color)
                }
            }
            TestResult::Skipped(reason) => {
                if reason.is_empty() {
                    style("SKIPPED", "33", use_color)
                } else {
                    format!("{} ({})", style("SKIPPED", "33", use_color), reason)
                }
            }
            TestResult::XFailed(_, reason) => {
                format!("{} ({})", style("XFAIL", "33", use_color), reason)
            }
            TestResult::XPassed(_) => style("XPASS", "31", use_color),
        };

        if self.verbose {
            eprintln!("{}", status);
        } else {
            eprint!("{}", status);
        }

        // Print failure details
        if let TestResult::Failed(_, error) = result {
            eprintln!("\n{}", style(&test.function_name, "31", use_color));
            eprintln!("{}", error);
        }
    }

    fn on_run_complete(&mut self, summary: &TestSummary) {
        let use_color = self.use_color;
        if !self.verbose {
            eprintln!();
        }
        eprintln!();

        let mut parts = Vec::new();
        if summary.passed > 0 {
            parts.push(style(format!("{} passed", summary.passed), "32", use_color));
        }
        if summary.failed > 0 {
            parts.push(style(format!("{} failed", summary.failed), "31", use_color));
        }
        if summary.skipped > 0 {
            parts.push(style(format!("{} skipped", summary.skipped), "33", use_color));
        }
        if summary.xfailed > 0 {
            parts.push(style(format!("{} xfailed", summary.xfailed), "33", use_color));
        }

        eprintln!(
            "====== {} in {:.2}s ======",
            parts.join(", "),
            summary.duration.as_secs_f64()
        );
    }
}

/// Information about a discovered test
#[derive(Debug, Clone)]
pub struct TestInfo {
    pub file_path: PathBuf,
    pub function_name: String,
    pub markers: Vec<TestMarker>,
    pub required_fixtures: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TestMarker {
    Skip(String),
    XFail(String),
    Slow,
    Parametrize(String, Vec<String>),
}

/// Fixture scope determines lifecycle
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FixtureScope {
    #[default]
    Function,
    Module,
    Session,
}

/// Information about a discovered fixture
#[derive(Debug, Clone)]
pub struct FixtureInfo {
    pub name: String,
    #[allow(dead_code)]
    pub file_path: PathBuf,
    pub scope: FixtureScope,
    pub autouse: bool,
    #[allow(dead_code)]
    pub dependencies: Vec<String>,
    pub has_teardown: bool,
    pub is_async: bool,
}

/// Result of running a single test
#[derive(Debug)]
pub enum TestResult {
    Passed(Duration),
    Failed(Duration, String),
    Skipped(String),
    XFailed(Duration, String),
    #[allow(dead_code)]
    XPassed(Duration),
}

/// Result of discovering tests and fixtures in a file
pub struct DiscoveryResult {
    pub tests: Vec<TestInfo>,
    pub fixtures: Vec<FixtureInfo>,
}

/// Run all tests in the given path.
pub fn run_tests(
    path: &str,
    verbose: bool,
    stop_on_fail: bool,
    include_slow: bool,
    filter: Option<&str>,
    use_color: bool,
    fail_on_empty: bool,
) -> CliResult<ExitCode> {
    let start_time = Instant::now();

    let test_files = discover_test_files(Path::new(path));

    if test_files.is_empty() {
        return Err(CliError::failure(format!(
            "No test files found in '{}'\nTest files should be named test_*.incn or *_test.incn",
            path
        )));
    }

    let mut all_tests: Vec<TestInfo> = Vec::new();
    let mut all_fixtures: HashMap<String, FixtureInfo> = HashMap::new();

    for file_path in &test_files {
        match discover_tests_and_fixtures(file_path) {
            Ok(result) => {
                all_tests.extend(result.tests);
                for fixture in result.fixtures {
                    all_fixtures.insert(fixture.name.clone(), fixture);
                }
            }
            Err(e) => {
                eprintln!("Error parsing {}: {}", file_path.display(), e);
            }
        }
    }

    if verbose && !all_fixtures.is_empty() {
        println!("Discovered {} fixture(s):", all_fixtures.len());
        for (name, fixture) in &all_fixtures {
            let scope_str = match fixture.scope {
                FixtureScope::Function => "function",
                FixtureScope::Module => "module",
                FixtureScope::Session => "session",
            };
            let autouse_str = if fixture.autouse { " (autouse)" } else { "" };
            let async_str = if fixture.is_async { " async" } else { "" };
            let teardown_str = if fixture.has_teardown { " (with teardown)" } else { "" };
            println!(
                "  - {}: scope={}{}{}{}",
                name, scope_str, autouse_str, async_str, teardown_str
            );
        }
        println!();
    }

    let autouse_fixtures = get_autouse_fixtures(&all_fixtures, FixtureScope::Function);

    let all_tests: Vec<TestInfo> = all_tests
        .into_iter()
        .map(|mut test| {
            for autouse in &autouse_fixtures {
                if !test.required_fixtures.contains(autouse) {
                    test.required_fixtures.push(autouse.clone());
                }
            }
            test
        })
        .collect();

    let filtered_tests: Vec<TestInfo> = all_tests
        .into_iter()
        .filter(|t| {
            if let Some(keyword) = filter {
                if !t.function_name.contains(keyword) {
                    return false;
                }
            }
            if !include_slow && t.markers.contains(&TestMarker::Slow) {
                return false;
            }
            true
        })
        .collect();

    if filtered_tests.is_empty() {
        eprintln!("No tests collected");
        if fail_on_empty {
            return Err(CliError::new("", ExitCode::FAILURE));
        }
        return Ok(ExitCode::SUCCESS); // "no tests collected" is not a failure
    }

    println!(
        "{}",
        style(
            "=================== test session starts ===================",
            "1",
            use_color
        )
    );
    println!("collected {} item(s)", filtered_tests.len());
    println!();

    let mut results: Vec<(TestInfo, TestResult)> = Vec::new();
    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    let mut xfailed = 0;
    let mut xpassed = 0;

    for test in filtered_tests {
        if let Some(TestMarker::Skip(reason)) = test.markers.iter().find(|m| matches!(m, TestMarker::Skip(_))) {
            let result = TestResult::Skipped(reason.clone());
            print_test_result(&test, &result, verbose, use_color);
            skipped += 1;
            results.push((test, result));
            continue;
        }

        let is_xfail = test.markers.iter().any(|m| matches!(m, TestMarker::XFail(_)));

        let result = run_single_test(&test);

        let result = if is_xfail {
            match result {
                TestResult::Passed(d) => {
                    xpassed += 1;
                    TestResult::XPassed(d)
                }
                TestResult::Failed(d, _) => {
                    let reason = test
                        .markers
                        .iter()
                        .find_map(|m| {
                            if let TestMarker::XFail(r) = m {
                                Some(r.clone())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default();
                    xfailed += 1;
                    TestResult::XFailed(d, reason)
                }
                other => other,
            }
        } else {
            match &result {
                TestResult::Passed(_) => passed += 1,
                TestResult::Failed(_, _) => failed += 1,
                _ => {}
            }
            result
        };

        print_test_result(&test, &result, verbose, use_color);

        if stop_on_fail && matches!(result, TestResult::Failed(_, _)) {
            results.push((test, result));
            break;
        }

        results.push((test, result));
    }

    let failures: Vec<_> = results
        .iter()
        .filter(|(_, r)| matches!(r, TestResult::Failed(_, _) | TestResult::XPassed(_)))
        .collect();

    if !failures.is_empty() {
        println!();
        println!(
            "{}",
            style("=================== FAILURES ===================", "1;31", use_color)
        );
        for (test, result) in failures {
            println!();
            println!(
                "{}",
                style(
                    format!("___________ {} ___________", test.function_name),
                    "1",
                    use_color
                )
            );
            if let TestResult::Failed(_, msg) = result {
                println!();
                println!("    {}", msg);
            } else if let TestResult::XPassed(_) = result {
                println!();
                println!(
                    "    {}",
                    style("Test passed but was expected to fail (xfail)", "33", use_color)
                );
            }
            println!();
            println!("    {}::{}", test.file_path.display(), test.function_name);
        }
    }

    let total_time = start_time.elapsed();
    println!();
    let summary_border = if failed > 0 || xpassed > 0 {
        // 31 is red
        style("===================", "1;31", use_color)
    } else {
        // 32 is green
        style("===================", "1;32", use_color)
    };
    print!("{}", summary_border);

    let mut parts = Vec::new();
    if passed > 0 {
        parts.push(format!("{} passed", passed));
    }
    if failed > 0 {
        parts.push(format!("{} failed", failed));
    }
    if skipped > 0 {
        parts.push(format!("{} skipped", skipped));
    }
    if xfailed > 0 {
        parts.push(format!("{} xfailed", xfailed));
    }
    if xpassed > 0 {
        parts.push(format!("{} xpassed", xpassed));
    }

    print!(" {} in {:.2}s ", parts.join(", "), total_time.as_secs_f64());
    println!("{}", summary_border);

    if failed > 0 || xpassed > 0 {
        // Tests failed - return error with empty message (summary already printed)
        Err(CliError::new("", ExitCode::FAILURE))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn print_test_result(test: &TestInfo, result: &TestResult, verbose: bool, use_color: bool) {
    let file_stem = test.file_path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown");

    let status = match result {
        TestResult::Passed(d) => {
            if verbose {
                format!("{} ({:.0}ms)", style("PASSED", "32", use_color), d.as_millis())
            } else {
                style("PASSED", "32", use_color)
            }
        }
        TestResult::Failed(d, _) => {
            if verbose {
                format!("{} ({:.0}ms)", style("FAILED", "31", use_color), d.as_millis())
            } else {
                style("FAILED", "31", use_color)
            }
        }
        TestResult::Skipped(reason) => {
            if reason.is_empty() {
                style("SKIPPED", "33", use_color)
            } else {
                format!("{} ({})", style("SKIPPED", "33", use_color), reason)
            }
        }
        TestResult::XFailed(_, reason) => {
            if reason.is_empty() {
                style("XFAIL", "33", use_color)
            } else {
                format!("{} ({})", style("XFAIL", "33", use_color), reason)
            }
        }
        TestResult::XPassed(_) => style("XPASS", "31", use_color),
    };

    println!("{}::{} {}", file_stem, test.function_name, status);
}

/// Discover test files in a directory
pub fn discover_test_files(path: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    if path.is_file() {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if (name.starts_with("test_") || name.ends_with("_test.incn")) && name.ends_with(".incn") {
            files.push(path.to_path_buf());
        }
    } else if path.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                if entry_path.is_dir() {
                    let name = entry_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') && name != "target" && name != "node_modules" {
                        files.extend(discover_test_files(&entry_path));
                    }
                } else {
                    let name = entry_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if (name.starts_with("test_") || name.ends_with("_test.incn")) && name.ends_with(".incn") {
                        files.push(entry_path);
                    }
                }
            }
        }
    }

    files.sort();
    files
}

/// Discover both tests and fixtures in a file
pub fn discover_tests_and_fixtures(file_path: &Path) -> Result<DiscoveryResult, String> {
    let source = fs::read_to_string(file_path).map_err(|e| format!("Failed to read file: {}", e))?;

    let tokens = lexer::lex(&source).map_err(|e| format!("Lexer error: {:?}", e))?;

    let ast = parser::parse(&tokens).map_err(|e| format!("Parser error: {:?}", e))?;

    let import_aliases = crate::frontend::decorator_resolution::collect_import_aliases(&ast);

    let mut tests = Vec::new();
    let mut fixtures = Vec::new();

    let fixture_names: Vec<String> = ast
        .declarations
        .iter()
        .filter_map(|decl| {
            if let crate::frontend::ast::Declaration::Function(func) = &decl.node {
                if has_fixture_decorator(&func.decorators, &import_aliases) {
                    return Some(func.name.clone());
                }
            }
            None
        })
        .collect();

    for decl in &ast.declarations {
        if let crate::frontend::ast::Declaration::Function(func) = &decl.node {
            if has_fixture_decorator(&func.decorators, &import_aliases) {
                let (scope, autouse) = extract_fixture_args(&func.decorators, &import_aliases);
                let dependencies = extract_fixture_dependencies(&func.params, &fixture_names);
                let has_teardown = function_has_yield(&func.body);

                fixtures.push(FixtureInfo {
                    name: func.name.clone(),
                    file_path: file_path.to_path_buf(),
                    scope,
                    autouse,
                    dependencies,
                    has_teardown,
                    is_async: func.is_async,
                });
            } else if func.name.starts_with("test_") {
                let markers = extract_test_markers(&func.decorators, &import_aliases);
                let required_fixtures = extract_fixture_dependencies(&func.params, &fixture_names);

                tests.push(TestInfo {
                    file_path: file_path.to_path_buf(),
                    function_name: func.name.clone(),
                    markers,
                    required_fixtures,
                });
            }
        }
    }

    Ok(DiscoveryResult { tests, fixtures })
}

fn has_fixture_decorator(
    decorators: &[crate::frontend::ast::Spanned<crate::frontend::ast::Decorator>],
    aliases: &HashMap<String, Vec<String>>,
) -> bool {
    decorators
        .iter()
        .any(|d| resolve_decorator_id(&d.node, aliases) == Some(DecoratorId::Fixture))
}

fn resolve_decorator_id(
    dec: &crate::frontend::ast::Decorator,
    aliases: &HashMap<String, Vec<String>>,
) -> Option<DecoratorId> {
    let resolved = crate::frontend::decorator_resolution::resolve_decorator_path(dec, aliases);
    decorators::from_segments(&resolved)
}

fn resolve_decorator_path_string(
    dec: &crate::frontend::ast::Decorator,
    aliases: &HashMap<String, Vec<String>>,
) -> String {
    let resolved = crate::frontend::decorator_resolution::resolve_decorator_path(dec, aliases);
    if resolved.is_empty() {
        return dec.name.clone();
    }
    resolved.join(".")
}

fn extract_fixture_args(
    decorators: &[crate::frontend::ast::Spanned<crate::frontend::ast::Decorator>],
    aliases: &HashMap<String, Vec<String>>,
) -> (FixtureScope, bool) {
    let mut scope = FixtureScope::default();
    let mut autouse = false;

    for dec in decorators {
        if resolve_decorator_id(&dec.node, aliases) == Some(DecoratorId::Fixture) {
            for arg in &dec.node.args {
                if let crate::frontend::ast::DecoratorArg::Named(name, value) = arg {
                    if name == decorators::FIXTURE_SCOPE_ARG {
                        if let crate::frontend::ast::DecoratorArgValue::Expr(expr) = value {
                            if let crate::frontend::ast::Expr::Literal(crate::frontend::ast::Literal::String(s)) =
                                &expr.node
                            {
                                scope = match s.as_str() {
                                    decorators::FIXTURE_SCOPE_FUNCTION => FixtureScope::Function,
                                    decorators::FIXTURE_SCOPE_MODULE => FixtureScope::Module,
                                    decorators::FIXTURE_SCOPE_SESSION => FixtureScope::Session,
                                    _ => FixtureScope::Function,
                                };
                            }
                        }
                    } else if name == decorators::FIXTURE_AUTOUSE_ARG {
                        if let crate::frontend::ast::DecoratorArgValue::Expr(expr) = value {
                            if let crate::frontend::ast::Expr::Literal(crate::frontend::ast::Literal::Bool(b)) =
                                &expr.node
                            {
                                autouse = *b;
                            }
                        }
                    }
                }
            }
        }
    }

    (scope, autouse)
}

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

fn function_has_yield(body: &[crate::frontend::ast::Spanned<crate::frontend::ast::Statement>]) -> bool {
    for stmt in body {
        if statement_has_yield(&stmt.node) {
            return true;
        }
    }
    false
}

fn statement_has_yield(stmt: &crate::frontend::ast::Statement) -> bool {
    match stmt {
        crate::frontend::ast::Statement::Expr(expr) => expr_has_yield(&expr.node),
        crate::frontend::ast::Statement::Return(Some(expr)) => expr_has_yield(&expr.node),
        crate::frontend::ast::Statement::If(if_stmt) => {
            if_stmt.then_body.iter().any(|s| statement_has_yield(&s.node))
                || if_stmt
                    .else_body
                    .as_ref()
                    .is_some_and(|b| b.iter().any(|s| statement_has_yield(&s.node)))
        }
        crate::frontend::ast::Statement::While(while_stmt) => {
            while_stmt.body.iter().any(|s| statement_has_yield(&s.node))
        }
        crate::frontend::ast::Statement::For(for_stmt) => for_stmt.body.iter().any(|s| statement_has_yield(&s.node)),
        _ => false,
    }
}

fn expr_has_yield(expr: &crate::frontend::ast::Expr) -> bool {
    match expr {
        crate::frontend::ast::Expr::Yield(_) => true,
        crate::frontend::ast::Expr::Binary(left, _, right) => expr_has_yield(&left.node) || expr_has_yield(&right.node),
        crate::frontend::ast::Expr::Unary(_, operand) => expr_has_yield(&operand.node),
        crate::frontend::ast::Expr::Call(callee, args) => {
            expr_has_yield(&callee.node)
                || args.iter().any(|a| match a {
                    crate::frontend::ast::CallArg::Positional(e) => expr_has_yield(&e.node),
                    crate::frontend::ast::CallArg::Named(_, e) => expr_has_yield(&e.node),
                })
        }
        crate::frontend::ast::Expr::Paren(inner) => expr_has_yield(&inner.node),
        _ => false,
    }
}

fn get_autouse_fixtures(fixtures: &HashMap<String, FixtureInfo>, scope: FixtureScope) -> Vec<String> {
    fixtures
        .values()
        .filter(|f| f.autouse && f.scope == scope)
        .map(|f| f.name.clone())
        .collect()
}

/// Extract test markers from the decorators.
/// FIXME: stringly typed lookup - this should be moved/removed
fn extract_test_markers(
    decorators: &[crate::frontend::ast::Spanned<crate::frontend::ast::Decorator>],
    aliases: &HashMap<String, Vec<String>>,
) -> Vec<TestMarker> {
    let mut markers = Vec::new();

    for dec in decorators {
        match resolve_decorator_path_string(&dec.node, aliases).as_str() {
            "std.testing.skip" => {
                let reason = extract_string_arg(&dec.node.args).unwrap_or_default();
                markers.push(TestMarker::Skip(reason));
            }
            "std.testing.xfail" => {
                let reason = extract_string_arg(&dec.node.args).unwrap_or_default();
                markers.push(TestMarker::XFail(reason));
            }
            "std.testing.slow" => {
                markers.push(TestMarker::Slow);
            }
            "std.testing.parametrize" => {
                markers.push(TestMarker::Parametrize(String::new(), Vec::new()));
            }
            _ => {}
        }
    }

    markers
}

fn extract_string_arg(args: &[crate::frontend::ast::DecoratorArg]) -> Option<String> {
    if let Some(crate::frontend::ast::DecoratorArg::Positional(expr)) = args.first() {
        if let crate::frontend::ast::Expr::Literal(crate::frontend::ast::Literal::String(s)) = &expr.node {
            return Some(s.clone());
        }
    }
    None
}

fn run_single_test(test: &TestInfo) -> TestResult {
    let start = Instant::now();

    let source = match fs::read_to_string(&test.file_path) {
        Ok(s) => s,
        Err(e) => {
            return TestResult::Failed(start.elapsed(), format!("Failed to read file: {}", e));
        }
    };

    let tokens = match lexer::lex(&source) {
        Ok(t) => t,
        Err(e) => return TestResult::Failed(start.elapsed(), format!("Lexer error: {:?}", e)),
    };

    let ast = match parser::parse(&tokens) {
        Ok(a) => a,
        Err(e) => return TestResult::Failed(start.elapsed(), format!("Parser error: {:?}", e)),
    };

    let mut codegen = IrCodegen::new();
    codegen.set_test_mode(true);
    codegen.set_test_function(&test.function_name);

    let rust_code = match codegen.try_generate(&ast) {
        Ok(code) => code,
        Err(e) => {
            return TestResult::Failed(start.elapsed(), format!("Code generation error: {}", e));
        }
    };

    let temp_dir = format!("target/incan_tests/{}", test.function_name);
    let generator = ProjectGenerator::new(&temp_dir, "test_runner", true);

    if let Err(e) = generator.generate(&rust_code) {
        return TestResult::Failed(start.elapsed(), format!("Failed to generate project: {}", e));
    }

    let output = std::process::Command::new("cargo")
        .arg("test")
        .arg("--")
        .arg("--nocapture")
        .current_dir(&temp_dir)
        .output();

    match output {
        Ok(output) => {
            let duration = start.elapsed();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if output.status.success() {
                TestResult::Passed(duration)
            } else {
                let msg = if stderr.contains("assertion") {
                    extract_assertion_error(&stderr)
                } else if stdout.contains("panicked") {
                    extract_panic_message(&stdout)
                } else {
                    format!("Test failed\n{}\n{}", stdout, stderr)
                };
                TestResult::Failed(duration, msg)
            }
        }
        Err(e) => TestResult::Failed(start.elapsed(), format!("Failed to run test: {}", e)),
    }
}

fn extract_assertion_error(stderr: &str) -> String {
    for line in stderr.lines() {
        if line.contains("assertion") || line.contains("AssertionError") {
            return line.trim().to_string();
        }
    }
    stderr.to_string()
}

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
