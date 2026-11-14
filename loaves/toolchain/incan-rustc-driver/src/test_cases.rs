//! Native entries for tests that take fixtures or `@parametrize` arguments.
//!
//! The native harness registers zero-argument functions, so a test that takes arguments runs through one generated
//! entry per case. Each entry sets up the test's fixtures by parameter name, passes the case's values and calls the
//! test, as 0.5.1 did: a fixture that depends on fixtures receives them as arguments, an autouse fixture runs after
//! the requested ones, and a fixture's setup runs again for every parameter that names it. A fixture that `yield`s
//! its value becomes one stage of the entry: the statements before `yield` set it up, the rest of the case runs
//! where it yielded, and the statements after it tear it down once that returns.
//!
//! The entries are Incan source appended to the test file and checked with it, so diagnostics, lowering and the
//! harness see ordinary functions. The test file's text is otherwise unchanged except that a `yield` fixture's own
//! declaration is blanked line for line: its stages replace it, and every later line keeps its number.

use std::collections::HashMap;
use std::path::Path;

use incan_driver::session::CompilationSession;
use incan_driver::testing::types::{DiscoveryResult, FixtureInfo, TestInfo, TestMarker};
use incan_frontend::ast::{Declaration, Expr, FunctionDecl, Spanned, Statement};

/// The prefix of every generated entry and local. Source names may not start with `__incan_`, which the checker
/// reserves for names it generates itself, so the source-level entries use this one.
const PREFIX: &str = "incan_case";

/// Runner-provided fixtures that 0.5.1 injects but the native harness does not provide yet.
const BUILTIN_FIXTURES: &[&str] = &["tmp_path", "tmp_workdir", "env"];

/// One case the harness runs: the name reports give it, the function it registers and the case's own markers.
pub(crate) struct NativeCase {
    /// Position of the case's test in the discovery result.
    pub(crate) test: usize,
    /// `test_add[1-2-3]` for a parametrized case, otherwise the test's name.
    pub(crate) name: String,
    /// The zero-argument function the harness registers.
    pub(crate) entry: String,
    /// Markers a `param_case(..., marks=[...])` adds to this case alone.
    pub(crate) markers: Vec<TestMarker>,
}

/// Every case of a test unit, and the test file's source with its generated entries when any case needs one.
pub(crate) struct NativeCases {
    pub(crate) cases: Vec<NativeCase>,
    pub(crate) source: Option<String>,
}

/// One parameter set of a parametrized test: its case id, each argument's source text by parameter name and its own
/// markers. A test without `@parametrize` has one row with no id.
#[derive(Clone, Default)]
struct Row {
    ids: Vec<String>,
    values: HashMap<String, String>,
    markers: Vec<TestMarker>,
}

/// Plan the native cases of a test unit.
///
/// 1. A unit without fixtures or test parameters keeps one case per test, registered under its own name.
/// 2. Otherwise read the test file's declarations, which must be top-level: inline `module tests:` keeps refusing.
/// 3. Expand each test into its parameter rows and write one entry per row.
/// 4. Return the test file's source with the entries appended and every `yield` fixture blanked.
pub(crate) fn plan(
    source_path: &Path,
    session: &CompilationSession,
    discovery: &DiscoveryResult,
) -> Result<NativeCases, String> {
    // 1. Plain tests register themselves.
    let needs_entries =
        !discovery.fixtures.is_empty() || discovery.tests.iter().any(|test| !test.parameter_names.is_empty());
    if !needs_entries {
        let cases = discovery
            .tests
            .iter()
            .enumerate()
            .map(|(index, test)| plain_case(index, test))
            .collect();
        return Ok(NativeCases { cases, source: None });
    }

    // 2. Entries are appended to a test file's top level.
    if !is_named_test_file(source_path) {
        return Err(
            "unsupported test unit fixtures or parameters in an inline test module on the native harness".into(),
        );
    }
    let source = std::fs::read_to_string(source_path)
        .map_err(|error| format!("cannot read test source {}: {error}", source_path.display()))?;
    let program = session
        .parse_source(source_path, &source, false)
        .map_err(|errors| format!("cannot parse test source {}: {errors:?}", source_path.display()))?;
    let mut functions = HashMap::new();
    for declaration in &program.declarations {
        if let Declaration::Function(function) = &declaration.node {
            functions.insert(function.name.clone(), (declaration, function));
        }
    }
    let writer = EntryWriter {
        source: &source,
        fixtures: discovery
            .fixtures
            .iter()
            .map(|fixture| (fixture.name.clone(), fixture))
            .collect(),
        functions: functions
            .iter()
            .map(|(name, (_, function))| (name.clone(), *function))
            .collect(),
    };
    writer.refuse_unsupported_fixtures(discovery)?;

    // 3. One entry per parameter row.
    let mut cases = Vec::new();
    let mut generated = String::new();
    for (index, test) in discovery.tests.iter().enumerate() {
        let autouse = autouse_fixtures(discovery, test);
        if test.parameter_names.is_empty() && autouse.is_empty() {
            cases.push(plain_case(index, test));
            continue;
        }
        if test.is_async {
            return Err(format!(
                "unsupported test contract for `{}`: an async test with fixtures or parameters",
                test.function_name
            ));
        }
        let function = writer.function(&test.function_name)?;
        if test
            .markers
            .iter()
            .any(|marker| matches!(marker, TestMarker::Parametrize(..)))
        {
            return Err(format!(
                "unsupported test contract for `{}`: parameters",
                test.function_name
            ));
        }
        for row in [Row::default()] {
            let entry = format!("{PREFIX}_{}", cases.len());
            generated.push_str(&writer.entry(&entry, test, function, &row, &autouse)?);
            let name = if row.ids.is_empty() {
                test.function_name.clone()
            } else {
                format!("{}[{}]", test.function_name, row.ids.join("-"))
            };
            cases.push(NativeCase {
                test: index,
                name,
                entry,
                markers: row.markers,
            });
        }
    }

    // 4. The checked source: the file, its `yield` fixtures blanked, then the entries.
    let mut checked = writer.blank_yield_fixtures(&functions)?;
    if !checked.ends_with('\n') {
        checked.push('\n');
    }
    checked.push_str(&generated);
    Ok(NativeCases {
        cases,
        source: Some(checked),
    })
}

/// A test that needs no entry registers under its own name.
fn plain_case(index: usize, test: &TestInfo) -> NativeCase {
    NativeCase {
        test: index,
        name: test.function_name.clone(),
        entry: test.function_name.clone(),
        markers: Vec::new(),
    }
}

/// Whether `path` is a standalone test file, whose tests and fixtures are top-level declarations.
fn is_named_test_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
    name.ends_with(".incn") && (name.starts_with("test_") || name.ends_with("_test.incn"))
}

/// The file's autouse fixtures a test does not already request, in declaration order.
fn autouse_fixtures<'a>(discovery: &'a DiscoveryResult, test: &TestInfo) -> Vec<&'a str> {
    discovery
        .fixtures
        .iter()
        .filter(|fixture| fixture.autouse && !test.parameter_names.contains(&fixture.name))
        .map(|fixture| fixture.name.as_str())
        .collect()
}

/// Writes entries from the test file's own text: fixture and test signatures, parameter values and `yield` bodies.
struct EntryWriter<'a> {
    source: &'a str,
    fixtures: HashMap<String, &'a FixtureInfo>,
    functions: HashMap<String, &'a FunctionDecl>,
}

impl<'a> EntryWriter<'a> {
    /// The source text between two byte offsets.
    fn text(&self, start: usize, end: usize) -> &'a str {
        self.source.get(start..end).unwrap_or("")
    }

    /// A function declared at the test file's top level.
    fn function(&self, name: &str) -> Result<&'a FunctionDecl, String> {
        self.functions
            .get(name)
            .copied()
            .ok_or_else(|| format!("collected test or fixture `{name}` is not a top-level function"))
    }

    /// Refuse fixtures the native harness cannot run yet, by name, before any entry is written.
    fn refuse_unsupported_fixtures(&self, discovery: &DiscoveryResult) -> Result<(), String> {
        for fixture in &discovery.fixtures {
            if fixture.is_async {
                return Err(format!(
                    "unsupported async fixture `{}` on the native harness",
                    fixture.name
                ));
            }
        }
        let requested = discovery
            .tests
            .iter()
            .flat_map(|test| test.required_fixtures.iter())
            .chain(
                discovery
                    .fixtures
                    .iter()
                    .flat_map(|fixture| fixture.dependencies.iter()),
            );
        for name in requested {
            if BUILTIN_FIXTURES.contains(&name.as_str()) && !self.fixtures.contains_key(name) {
                return Err(format!("unsupported built-in fixture `{name}` on the native harness"));
            }
        }
        Ok(())
    }

    /// The expression that sets up a fixture for one use: a `yield` fixture's staged value, or a call that sets up
    /// its own dependencies first, again for each use as 0.5.1 does.
    fn setup(&self, name: &str, staged: &[&str], depth: usize) -> Result<String, String> {
        if staged.contains(&name) {
            return Ok(name.to_string());
        }
        let fixture = self
            .fixtures
            .get(name)
            .ok_or_else(|| format!("missing fixture `{name}`"))?;
        if depth > self.fixtures.len() {
            return Err(format!("fixture dependency cycle through `{name}`"));
        }
        let arguments = fixture
            .dependencies
            .iter()
            .map(|dependency| self.setup(dependency, staged, depth + 1))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(format!("{name}({})", arguments.join(", ")))
    }

    /// The `yield` fixtures a case uses, each after the `yield` fixtures it depends on.
    fn stages(&self, roots: &[&str]) -> Result<Vec<&'a FixtureInfo>, String> {
        let mut ordered: Vec<&'a FixtureInfo> = Vec::new();
        let mut pending: Vec<(&str, usize)> = roots.iter().rev().map(|root| (*root, 0)).collect();
        let mut visiting: Vec<String> = Vec::new();
        while let Some((name, depth)) = pending.pop() {
            let Some(fixture) = self.fixtures.get(name).copied() else {
                continue;
            };
            if ordered.iter().any(|staged| staged.name == fixture.name) {
                continue;
            }
            if depth > self.fixtures.len() {
                return Err(format!("fixture dependency cycle through `{name}`"));
            }
            let open = fixture
                .dependencies
                .iter()
                .filter(|dependency| {
                    self.fixtures.contains_key(dependency.as_str())
                        && !ordered.iter().any(|staged| &staged.name == *dependency)
                })
                .collect::<Vec<_>>();
            if open.is_empty() || visiting.contains(&fixture.name) {
                visiting.retain(|visited| visited != &fixture.name);
                if fixture.has_teardown {
                    ordered.push(fixture);
                }
                continue;
            }
            visiting.push(fixture.name.clone());
            pending.push((name, depth + 1));
            for dependency in open.into_iter().rev() {
                pending.push((dependency.as_str(), depth + 1));
            }
        }
        Ok(ordered)
    }

    /// Write one case's entry: a stage per `yield` fixture, outermost first, then the stage that calls the test.
    ///
    /// Stage `k` receives the values of the `yield` fixtures before it and sets up its fixture's other dependencies;
    /// its body is the fixture's own statements, with the next stage called where the fixture yields.
    fn entry(
        &self,
        entry: &str,
        test: &TestInfo,
        function: &FunctionDecl,
        row: &Row,
        autouse: &[&str],
    ) -> Result<String, String> {
        let returns = return_type(self, function);
        let returns_value = returns != "None";
        let mut roots: Vec<&str> = test
            .parameter_names
            .iter()
            .filter(|name| !row.values.contains_key(name.as_str()))
            .map(String::as_str)
            .collect();
        roots.extend(autouse.iter().copied());
        let stages = self.stages(&roots)?;
        let stage_name = |position: usize| {
            if position == 0 {
                entry.to_string()
            } else {
                format!("{entry}_stage_{position}")
            }
        };
        let mut written = String::new();
        let mut staged: Vec<&str> = Vec::new();
        let mut bound: Vec<String> = Vec::new();
        for (position, fixture) in stages.iter().enumerate() {
            let stage = Stage {
                name: stage_name(position),
                next: stage_name(position + 1),
                returns: returns.clone(),
                returns_value,
            };
            written.push_str(&self.yield_stage(&stage, fixture, &staged, &bound)?);
            staged.push(fixture.name.as_str());
            bound.push(format!(
                "{}: {}",
                fixture.name,
                return_type(self, self.function(&fixture.name)?)
            ));
        }
        let stage = Stage {
            name: stage_name(stages.len()),
            next: String::new(),
            returns,
            returns_value,
        };
        written.push_str(&self.call_stage(&stage, test, row, autouse, &staged, &bound)?);
        Ok(written)
    }

    /// The last stage: set up the remaining fixtures in parameter order, run the autouse fixtures, call the test.
    fn call_stage(
        &self,
        stage: &Stage,
        test: &TestInfo,
        row: &Row,
        autouse: &[&str],
        staged: &[&str],
        bound: &[String],
    ) -> Result<String, String> {
        let mut body = String::new();
        let mut arguments = Vec::new();
        for (position, parameter) in test.parameter_names.iter().enumerate() {
            if let Some(value) = row.values.get(parameter) {
                arguments.push(value.clone());
            } else if staged.contains(&parameter.as_str()) {
                arguments.push(parameter.clone());
            } else {
                let local = format!("{PREFIX}_argument_{position}");
                body.push_str(&format!("    {local} = {}\n", self.setup(parameter, staged, 0)?));
                arguments.push(local);
            }
        }
        for fixture in autouse {
            if !staged.contains(fixture) {
                body.push_str(&format!("    {}\n", self.setup(fixture, staged, 0)?));
            }
        }
        body.push_str(&format!("    {}\n", stage.call(&test.function_name, &arguments)));
        Ok(format!(
            "\n\ndef {}({}) -> {}:\n{body}",
            stage.name,
            bound.join(", "),
            stage.returns
        ))
    }

    /// One `yield` fixture's stage, as two functions. The stage itself sets up the fixture's dependencies that are not
    /// staged values and passes them on; its body is the fixture's statements up to `yield`, the next stage called
    /// with the staged values and the yielded one, then the fixture's teardown statements, returning the next stage's
    /// outcome.
    fn yield_stage(
        &self,
        stage: &Stage,
        fixture: &FixtureInfo,
        staged: &[&str],
        bound: &[String],
    ) -> Result<String, String> {
        let function = self.function(&fixture.name)?;
        let (yielded, value) = yielded_value(function, &fixture.name)?;
        let (Some(first), Some(last)) = (function.body.first(), function.body.last()) else {
            return Err(format!("fixture `{}` has no body", fixture.name));
        };

        // The stage: the staged values, then the fixture's other dependencies, set up here.
        let body_name = format!("{}_body", stage.name);
        let mut parameters = bound.to_vec();
        let mut arguments: Vec<String> = staged.iter().map(|name| name.to_string()).collect();
        for parameter in &function.params {
            let name = parameter.node.name.as_str();
            if staged.contains(&name) {
                continue;
            }
            let ty = self.text(parameter.node.ty.span.start, parameter.node.ty.span.end);
            parameters.push(format!("{name}: {ty}"));
            arguments.push(self.setup(name, staged, 0)?);
        }
        let mut written = format!(
            "\n\ndef {}({}) -> {}:\n    {}\n",
            stage.name,
            bound.join(", "),
            stage.returns,
            stage.call(&body_name, &arguments)
        );

        // The body: the fixture's statements, the next stage where it yields.
        let indent = self.text(self.line_start(yielded.span.start), yielded.span.start);
        let mut next_arguments: Vec<String> = staged.iter().map(|name| name.to_string()).collect();
        next_arguments.push(self.text(value.span.start, value.span.end).to_string());
        let next = format!("{}({})", stage.next, next_arguments.join(", "));
        written.push_str(&format!(
            "\n\ndef {body_name}({}) -> {}:\n",
            parameters.join(", "),
            stage.returns
        ));
        written.push_str(self.text(self.line_start(first.span.start), yielded.span.start));
        if stage.returns_value {
            written.push_str(&format!("{PREFIX}_outcome = {next}"));
        } else {
            written.push_str(&next);
        }
        written.push_str(self.text(yielded.span.end, self.line_end(last.span.end)));
        if stage.returns_value {
            written.push_str(&format!("\n{indent}return {PREFIX}_outcome"));
        }
        written.push('\n');
        Ok(written)
    }
}

/// One stage function of an entry: its name, the stage it hands on to, and the test's return type.
struct Stage {
    name: String,
    next: String,
    returns: String,
    returns_value: bool,
}

impl Stage {
    /// A call statement that returns the callee's outcome when the test returns one.
    fn call(&self, callee: &str, arguments: &[String]) -> String {
        let call = format!("{callee}({})", arguments.join(", "));
        if self.returns_value {
            format!("return {call}")
        } else {
            call
        }
    }
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

/// A `yield` fixture's top-level `yield` statement and the value it yields.
fn yielded_value<'f>(
    function: &'f FunctionDecl,
    name: &str,
) -> Result<(&'f Spanned<Statement>, &'f Spanned<Expr>), String> {
    for statement in &function.body {
        if let Statement::Expr(expr) = &statement.node
            && let Expr::Yield(yielded) = &expr.node
        {
            return match yielded {
                Some(value) => Ok((statement, &**value)),
                None => Err(format!(
                    "fixture `{name}` uses `yield` teardown without a yielded value; fixtures must yield the fixture value"
                )),
            };
        }
    }
    Err(format!("fixture `{name}` has no top-level `yield`"))
}

impl EntryWriter<'_> {
    /// The offset where the line holding `offset` starts.
    fn line_start(&self, offset: usize) -> usize {
        self.source
            .get(..offset)
            .and_then(|before| before.rfind('\n'))
            .map_or(0, |newline| newline + 1)
    }

    /// The offset where the line holding `offset` ends, before its newline.
    fn line_end(&self, offset: usize) -> usize {
        self.source
            .get(offset..)
            .and_then(|after| after.find('\n'))
            .map_or(self.source.len(), |newline| offset + newline)
    }

    /// The file's text with each `yield` fixture's decorators and declaration blanked, keeping every line break.
    fn blank_yield_fixtures(
        &self,
        functions: &HashMap<String, (&Spanned<Declaration>, &FunctionDecl)>,
    ) -> Result<String, String> {
        let mut regions = Vec::new();
        for fixture in self.fixtures.values().filter(|fixture| fixture.has_teardown) {
            let (declaration, function) = functions
                .get(&fixture.name)
                .ok_or_else(|| format!("fixture `{}` is not a top-level function", fixture.name))?;
            let mut start = function
                .decorators
                .iter()
                .map(|decorator| decorator.span.start)
                .fold(declaration.span.start, usize::min);
            if start > 0 && self.source.as_bytes().get(start - 1) == Some(&b'@') {
                start -= 1;
            }
            regions.push((start, declaration.span.end));
        }
        let mut blanked = String::with_capacity(self.source.len());
        for (offset, character) in self.source.char_indices() {
            let hidden = regions.iter().any(|(start, end)| offset >= *start && offset < *end);
            blanked.push(if hidden && character != '\n' { ' ' } else { character });
        }
        Ok(blanked)
    }
}
