//! Behaviour fixtures: Incan programs that carry their own expected observables, and the runner that proves them.
//!
//! A behaviour fixture is the route-agnostic twin of a retire-class test (#1561, test corpus). It is an Incan program
//! under `loaves/compiler/incan_test_support/fixtures/behavior/<area>/` whose leading comment block says what the
//! program proves, which retire tests it stands in for, and what a run of it must show: its stdout, its exit code, or
//! the diagnostic that must refuse it. Nothing in the format names a route: today the runner drives the program
//! through `incan run` (the legacy generated-Rust route) and `incan check`; when slice 7 flips the route underneath,
//! the fixtures do not change. The contributor-facing description of the format is the family's `README.md`.
//!
//! The runner discovers every fixture in an area, materializes each one as a scratch project under
//! `INCAN_TEST_TMP_ROOT`, runs it, compares the observables and reports **every** failing fixture with its path and
//! its expected-versus-actual, so one libtest case per area still attributes a failure to the fixture that caused it.
//! Roots are thin: `loaves/toolchain/incan-cli/tests/behavior_<area>_tests.rs` calls [`assert_area_green`] and nothing
//! else. The inventory (`scripts/test_inventory/collect.py`) reads the same header for its `# retires:` lines, so the
//! header grammar here and the collector's reader must agree.

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use crate::cli_project::{run_explicit_oven_bake, run_incan, write_minimal_project};

/// Directory under [`crate::fixtures_dir`] that holds every behaviour-fixture area.
pub const BEHAVIOR_FIXTURES_ROOT: &str = "behavior";

/// The entrypoint every materialized fixture project runs and checks.
const ENTRYPOINT: &str = "src/main.incn";

/// The two block directives, whose lines follow them indented.
const EXPECT_STDOUT: &str = "expect-stdout";
const EXPECT_STDOUT_CONTAINS: &str = "expect-stdout-contains";

/// The directives a fixture header may carry, in the order the README documents them.
const DIRECTIVES: &[&str] = &[
    "behavior",
    "retires",
    EXPECT_STDOUT,
    EXPECT_STDOUT_CONTAINS,
    "expect-exit",
    "expect-diagnostic",
];

// ============================================================
// Format
// ============================================================

/// What a run of the fixture's stdout must show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdoutExpectation {
    /// The header declared no stdout expectation; only the exit code is checked.
    Unchecked,
    /// Every line of stdout, in order (`# expect-stdout:`).
    Exact(Vec<String>),
    /// Lines that must each appear as a whole line of stdout, in any order (`# expect-stdout-contains:`).
    Contains(Vec<String>),
}

/// The observables a fixture declares: either a run with its stdout and exit code, or a refusal at check time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expectation {
    /// The program must build and run; its stdout and exit code are compared.
    Run {
        /// The stdout expectation, exact or by contained lines.
        stdout: StdoutExpectation,
        /// The exit code the run must end with (`# expect-exit:`, default 0).
        exit_code: i32,
    },
    /// The program must be refused by `incan check` with every listed diagnostic code; it is never run.
    Refused {
        /// Diagnostic codes such as `INCAN-T0001`, each of which must appear in the check report.
        diagnostics: Vec<String>,
    },
}

/// The parsed leading comment block of a fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// One line saying what the program proves (`# behavior:`).
    pub behavior: String,
    /// The retire-class tests this fixture is the twin of, as `path::fn` keys the inventory recognises (`# retires:`).
    pub retires: Vec<String>,
    /// What a run must show.
    pub expectation: Expectation,
}

/// How a fixture is laid out on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureLayout {
    /// One `<name>.incn` file: the whole program, header first.
    SingleFile,
    /// A `<name>/` directory of modules with the header in `main.incn`; the modules become the project's `src/`.
    Modules,
    /// A `<name>/` directory that is a complete project (it has a `loaf.toml`); copied as it is, header in
    /// `src/main.incn`.
    Project,
}

/// One discovered and parsed behaviour fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehaviorFixture {
    /// The fixture file or directory, absolute.
    pub path: PathBuf,
    /// The fixture's name: the file stem or the directory name.
    pub name: String,
    /// Where the fixture's program lives relative to `path`.
    pub layout: FixtureLayout,
    /// The parsed header.
    pub header: Header,
}

impl BehaviorFixture {
    /// The path the inventory and failure reports spell: relative to the checkout, with `/` separators.
    pub fn display_path(&self) -> String {
        checkout_relative(&self.path)
    }

    /// The file that carries the header, by layout.
    fn header_file(path: &Path, layout: FixtureLayout) -> PathBuf {
        match layout {
            FixtureLayout::SingleFile => path.to_path_buf(),
            FixtureLayout::Modules => path.join("main.incn"),
            FixtureLayout::Project => path.join(ENTRYPOINT),
        }
    }
}

/// Why a fixture could not be read: the path and the reason, so a malformed fixture names itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureFormatError {
    /// The fixture (checkout-relative when it is under the checkout).
    pub path: String,
    /// What is wrong with it.
    pub reason: String,
}

impl fmt::Display for FixtureFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.reason)
    }
}

impl Error for FixtureFormatError {}

/// Spell a path relative to the checkout when it is under it, otherwise as given.
fn checkout_relative(path: &Path) -> String {
    let root = crate::repo_root();
    path.strip_prefix(&root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Parse the leading comment block of a fixture program.
///
/// The header is the run of lines at the top of the file that start with `#`; the first line that does not ends it.
/// Every header line is a directive (`# key: value`), an item of the block directive above it (`#` followed by two or
/// more spaces, or a bare `#` for an empty expected line), or a bare `#` between directives, which is ignored.
/// Anything else is refused with the line number, and so is an unknown directive, a `behavior:` that is missing or
/// repeated, a stdout block beside another stdout block, a `expect-diagnostic:` beside any run expectation, and a
/// header that declares no observable at all: a fixture that proves nothing is not a twin.
pub fn parse_header(text: &str) -> Result<Header, String> {
    let mut behavior: Option<String> = None;
    let mut retires: Vec<String> = Vec::new();
    let mut stdout_exact: Option<Vec<String>> = None;
    let mut stdout_contains: Option<Vec<String>> = None;
    let mut exit_code: Option<i32> = None;
    let mut diagnostics: Vec<String> = Vec::new();
    // The block directive currently collecting items, with the indentation its first item established.
    let mut open_block: Option<(&'static str, Option<usize>)> = None;

    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let Some(rest) = line.strip_prefix('#') else {
            break;
        };

        // ---- Bare `#`: an empty expected line inside a block, a separator outside one ----
        if rest.trim().is_empty() {
            if let Some((key, _)) = open_block {
                push_block_item(key, String::new(), &mut stdout_exact, &mut stdout_contains);
            }
            continue;
        }

        // ---- Block item: `#` followed by two or more spaces ----
        let indent = rest.len() - rest.trim_start_matches(' ').len();
        if indent >= 2 {
            let Some((key, established)) = open_block.as_mut() else {
                return Err(format!(
                    "line {number}: indented line outside an `expect-stdout:` / `expect-stdout-contains:` block"
                ));
            };
            let width = *established.get_or_insert(indent);
            let item = rest.get(width.min(indent)..).unwrap_or("").to_string();
            push_block_item(key, item, &mut stdout_exact, &mut stdout_contains);
            continue;
        }

        // ---- Directive: `# key: value` ----
        let directive = rest.trim_start();
        let Some((raw_key, raw_value)) = directive.split_once(':') else {
            return Err(format!(
                "line {number}: `#{rest}` is neither a directive (`# key: value`) nor an indented block item; put prose in `behavior:`"
            ));
        };
        let key = raw_key.trim();
        let value = raw_value.trim();
        open_block = None;
        match key {
            "behavior" => {
                if behavior.is_some() {
                    return Err(format!("line {number}: `behavior:` is declared twice"));
                }
                if value.is_empty() {
                    return Err(format!("line {number}: `behavior:` must say what the program proves"));
                }
                behavior = Some(value.to_string());
            }
            "retires" => {
                let Some((path, test)) = value.split_once("::") else {
                    return Err(format!(
                        "line {number}: `retires:` must name a test as `<path>.rs::<fn>`, got `{value}`"
                    ));
                };
                if !path.ends_with(".rs") || test.is_empty() || test.contains(char::is_whitespace) {
                    return Err(format!(
                        "line {number}: `retires:` must name a test as `<path>.rs::<fn>`, got `{value}`"
                    ));
                }
                if retires.iter().any(|existing| existing == value) {
                    return Err(format!("line {number}: `retires: {value}` is declared twice"));
                }
                retires.push(value.to_string());
            }
            "expect-stdout" | "expect-stdout-contains" => {
                if !value.is_empty() {
                    return Err(format!(
                        "line {number}: `{key}:` takes its lines below it, indented by two spaces after `#`"
                    ));
                }
                let (block, slot) = if key == "expect-stdout" {
                    (EXPECT_STDOUT, &mut stdout_exact)
                } else {
                    (EXPECT_STDOUT_CONTAINS, &mut stdout_contains)
                };
                if slot.is_some() {
                    return Err(format!("line {number}: `{key}:` is declared twice"));
                }
                *slot = Some(Vec::new());
                open_block = Some((block, None));
            }
            "expect-exit" => {
                if exit_code.is_some() {
                    return Err(format!("line {number}: `expect-exit:` is declared twice"));
                }
                let code = value
                    .parse::<i32>()
                    .map_err(|_| format!("line {number}: `expect-exit:` must be an integer, got `{value}`"))?;
                exit_code = Some(code);
            }
            "expect-diagnostic" => {
                if value.is_empty() || value.contains(char::is_whitespace) {
                    return Err(format!(
                        "line {number}: `expect-diagnostic:` must name one diagnostic code such as `INCAN-T0001`"
                    ));
                }
                if diagnostics.iter().any(|existing| existing == value) {
                    return Err(format!("line {number}: `expect-diagnostic: {value}` is declared twice"));
                }
                diagnostics.push(value.to_string());
            }
            other => {
                return Err(format!(
                    "line {number}: unknown directive `{other}`; the header accepts {}",
                    DIRECTIVES
                        .iter()
                        .map(|d| format!("`{d}:`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
    }

    // ---- Cross-directive rules ----
    let behavior = behavior.ok_or_else(|| "the header has no `behavior:` line".to_string())?;
    if stdout_exact.is_some() && stdout_contains.is_some() {
        return Err("`expect-stdout:` and `expect-stdout-contains:` cannot both be declared; pick one".to_string());
    }
    let declares_run = stdout_exact.is_some() || stdout_contains.is_some() || exit_code.is_some();
    if !diagnostics.is_empty() {
        if declares_run {
            return Err(
                "`expect-diagnostic:` means the program is refused at check time and never run; drop the `expect-stdout` / `expect-exit` lines"
                    .to_string(),
            );
        }
        return Ok(Header {
            behavior,
            retires,
            expectation: Expectation::Refused { diagnostics },
        });
    }
    if !declares_run {
        return Err(
            "the header declares no observable: add `expect-stdout:`, `expect-stdout-contains:`, `expect-exit:` or `expect-diagnostic:`"
                .to_string(),
        );
    }
    let stdout = if let Some(lines) = stdout_exact {
        StdoutExpectation::Exact(lines)
    } else if let Some(lines) = stdout_contains {
        StdoutExpectation::Contains(lines)
    } else {
        StdoutExpectation::Unchecked
    };
    Ok(Header {
        behavior,
        retires,
        expectation: Expectation::Run {
            stdout,
            exit_code: exit_code.unwrap_or(0),
        },
    })
}

/// Append one item to whichever stdout block is open.
fn push_block_item(
    key: &str,
    item: String,
    stdout_exact: &mut Option<Vec<String>>,
    stdout_contains: &mut Option<Vec<String>>,
) {
    let slot = if key == EXPECT_STDOUT {
        stdout_exact
    } else {
        stdout_contains
    };
    if let Some(lines) = slot.as_mut() {
        lines.push(item);
    }
}

// ============================================================
// Discovery
// ============================================================

/// The directory of one behaviour-fixture area, such as `behavior/smoke`.
pub fn area_dir(area: &str) -> PathBuf {
    crate::fixture(BEHAVIOR_FIXTURES_ROOT).join(area)
}

/// Discover and parse every fixture in an area directory, in name order.
///
/// An entry is a single-file fixture (`<name>.incn`), a module directory (`<name>/main.incn`) or a project directory
/// (`<name>/loaf.toml`). Hidden entries are skipped; anything else is refused, because a stray file in an area
/// would otherwise silently prove nothing. An area with no fixture at all is refused for the same reason.
pub fn discover(area_dir: &Path) -> Result<Vec<BehaviorFixture>, FixtureFormatError> {
    let area_display = checkout_relative(area_dir);
    let entries = fs::read_dir(area_dir).map_err(|error| FixtureFormatError {
        path: area_display.clone(),
        reason: format!("cannot read the area directory: {error}"),
    })?;
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| FixtureFormatError {
            path: area_display.clone(),
            reason: format!("cannot read an area entry: {error}"),
        })?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        paths.push(entry.path());
    }
    paths.sort();

    let mut fixtures = Vec::with_capacity(paths.len());
    for path in paths {
        fixtures.push(parse_fixture(&path)?);
    }
    if fixtures.is_empty() {
        return Err(FixtureFormatError {
            path: area_display,
            reason: "the area has no fixture; an empty area proves nothing".to_string(),
        });
    }
    Ok(fixtures)
}

/// Parse one fixture from its file or directory.
pub fn parse_fixture(path: &Path) -> Result<BehaviorFixture, FixtureFormatError> {
    let display = checkout_relative(path);
    let refuse = |reason: String| FixtureFormatError {
        path: display.clone(),
        reason,
    };
    let layout = if path.is_dir() {
        if path.join("loaf.toml").is_file() {
            FixtureLayout::Project
        } else if path.join("main.incn").is_file() {
            FixtureLayout::Modules
        } else {
            return Err(refuse(
                "a fixture directory needs a `main.incn` (modules) or a `loaf.toml` with `src/main.incn` (project)"
                    .to_string(),
            ));
        }
    } else if path.extension().is_some_and(|extension| extension == "incn") {
        FixtureLayout::SingleFile
    } else {
        return Err(refuse(
            "not a fixture: expected `<name>.incn` or a `<name>/` directory".to_string(),
        ));
    };
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| refuse("the fixture has no name".to_string()))?;
    let header_file = BehaviorFixture::header_file(path, layout);
    let text = fs::read_to_string(&header_file)
        .map_err(|error| refuse(format!("cannot read `{}`: {error}", checkout_relative(&header_file))))?;
    let header = parse_header(&text).map_err(|reason| FixtureFormatError {
        path: checkout_relative(&header_file),
        reason,
    })?;
    Ok(BehaviorFixture {
        path: path.to_path_buf(),
        name,
        layout,
        header,
    })
}

// ============================================================
// Running
// ============================================================

/// One fixture that did not show what its header declared, with everything a reader needs to see why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureFailure {
    /// The fixture, checkout-relative.
    pub path: String,
    /// The fixture's `behavior:` line.
    pub behavior: String,
    /// The mismatch, expected against actual, already laid out for a report.
    pub detail: String,
}

impl fmt::Display for FixtureFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "--- {}", self.path)?;
        writeln!(f, "behavior: {}", self.behavior)?;
        write!(f, "{}", self.detail)
    }
}

/// What one fixture showed when it ran: what its header declared, or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Every declared observable matched.
    Passed,
    /// At least one observable did not; the failure says which.
    Failed(FixtureFailure),
}

/// What running an area produced: every fixture attempted, and every one that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AreaReport {
    /// The area directory, checkout-relative.
    pub area: String,
    /// Checkout-relative paths of the fixtures that showed what they declared.
    pub passed: Vec<String>,
    /// Every fixture that did not, in discovery order.
    pub failures: Vec<FixtureFailure>,
}

impl AreaReport {
    /// The report as one message: a headline, then each failure with its expected-versus-actual.
    pub fn message(&self) -> String {
        let mut out = format!(
            "{} of {} behaviour fixture(s) failed in {}\n",
            self.failures.len(),
            self.passed.len() + self.failures.len(),
            self.area
        );
        for failure in &self.failures {
            out.push('\n');
            out.push_str(&failure.to_string());
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
        if !self.passed.is_empty() {
            out.push_str("\npassed:\n");
            for path in &self.passed {
                out.push_str("  ");
                out.push_str(path);
                out.push('\n');
            }
        }
        out
    }
}

/// The directory scratch projects are created under: `INCAN_TEST_TMP_ROOT` when the harness set it, else the
/// process temporary directory. Created when missing so a fresh managed root works on first use.
pub fn scratch_root() -> Result<PathBuf, Box<dyn Error>> {
    let root = std::env::var_os("INCAN_TEST_TMP_ROOT")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&root)?;
    Ok(root)
}

/// Materialize the fixture as a project at `project_root` and return the entrypoint the commands take.
///
/// A single file becomes `src/main.incn` of the minimal project; a module directory's files become `src/`; a project
/// directory is copied as it is. The header comments travel with the program: they are Incan comments.
pub fn materialize(fixture: &BehaviorFixture, project_root: &Path) -> Result<(), Box<dyn Error>> {
    let project_name = fixture
        .name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>();
    match fixture.layout {
        FixtureLayout::SingleFile => {
            let main = write_minimal_project(project_root, &project_name, "")?;
            fs::copy(&fixture.path, main)?;
        }
        FixtureLayout::Modules => {
            write_minimal_project(project_root, &project_name, "")?;
            crate::cli_project::copy_fixture_directory(&fixture.path, &project_root.join("src"))?;
        }
        FixtureLayout::Project => {
            crate::cli_project::copy_fixture_directory(&fixture.path, project_root)?;
        }
    }
    Ok(())
}

/// Run one fixture in its own scratch project and compare what it showed with what its header declares.
///
/// A refused-program fixture goes through `incan check --format json` only. A run fixture is baked first outside the
/// compiler suite (a fresh project has no Loaf authority until then; under the suite the sealed stdlib Loaf serves)
/// and then run through `incan run`, which is the legacy route today and whatever slice 7 makes it tomorrow.
pub fn run_fixture(fixture: &BehaviorFixture, scratch_root: &Path) -> Result<Outcome, Box<dyn Error>> {
    let project = tempfile::Builder::new()
        .prefix(&format!("behavior-{}-", fixture.name))
        .tempdir_in(scratch_root)?;
    materialize(fixture, project.path())?;
    let failure = |detail: String| FixtureFailure {
        path: fixture.display_path(),
        behavior: fixture.header.behavior.clone(),
        detail,
    };

    let outcome = |compared: Result<(), String>| match compared {
        Ok(()) => Outcome::Passed,
        Err(detail) => Outcome::Failed(failure(detail)),
    };

    match &fixture.header.expectation {
        Expectation::Refused { diagnostics } => {
            let check = run_incan(project.path(), &["check", ENTRYPOINT, "--format", "json"])?;
            Ok(outcome(compare_refusal(diagnostics, &check)))
        }
        Expectation::Run { stdout, exit_code } => {
            if !crate::oven_compiler_suite_is_active() {
                let bake = run_explicit_oven_bake(project.path())?;
                if !bake.status.success() {
                    return Ok(outcome(Err(format!(
                        "the standalone bake that prepares the project failed (exit {})\nstdout:\n{}stderr:\n{}",
                        describe_status(&bake),
                        indent_block(&String::from_utf8_lossy(&bake.stdout)),
                        indent_block(&String::from_utf8_lossy(&bake.stderr))
                    ))));
                }
            }
            let run = run_incan(project.path(), &["run", ENTRYPOINT])?;
            Ok(outcome(compare_run(stdout, *exit_code, &run)))
        }
    }
}

/// Compare a check report with the diagnostics a refused fixture declares.
fn compare_refusal(expected: &[String], check: &Output) -> Result<(), String> {
    let stdout = String::from_utf8_lossy(&check.stdout);
    let stderr = String::from_utf8_lossy(&check.stderr);
    if check.status.success() {
        return Err(format!(
            "expected `incan check` to refuse the program with {}, but it accepted it\nstdout:\n{}stderr:\n{}",
            list_codes(expected),
            indent_block(&stdout),
            indent_block(&stderr)
        ));
    }
    let report: serde_json::Value = serde_json::from_str(&stdout).map_err(|error| {
        format!(
            "expected `incan check --format json` to report {}, but its stdout is not a JSON report ({error})\nstdout:\n{}stderr:\n{}",
            list_codes(expected),
            indent_block(&stdout),
            indent_block(&stderr)
        )
    })?;
    let reported: Vec<String> = report["diagnostics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|diagnostic| diagnostic["code"].as_str().map(str::to_string))
        .collect();
    let missing: Vec<&String> = expected.iter().filter(|code| !reported.contains(code)).collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "expected diagnostics: {}\nmissing: {}\nreported: {}\nstderr:\n{}",
        list_codes(expected),
        missing.iter().map(|code| code.as_str()).collect::<Vec<_>>().join(", "),
        if reported.is_empty() {
            "(none)".to_string()
        } else {
            reported.join(", ")
        },
        indent_block(&stderr)
    ))
}

/// Compare a run's stdout and exit code with what a run fixture declares.
fn compare_run(stdout: &StdoutExpectation, exit_code: i32, run: &Output) -> Result<(), String> {
    let actual_stdout = String::from_utf8_lossy(&run.stdout);
    let actual_lines: Vec<&str> = actual_stdout.lines().collect();
    let mut problems: Vec<String> = Vec::new();

    // ---- Exit code ----
    let actual_code = run.status.code();
    if actual_code != Some(exit_code) {
        problems.push(format!("exit code: expected {exit_code}, got {}", describe_status(run)));
    }

    // ---- Stdout ----
    match stdout {
        StdoutExpectation::Unchecked => {}
        StdoutExpectation::Exact(expected) => {
            if expected.iter().map(String::as_str).ne(actual_lines.iter().copied()) {
                problems.push(format!(
                    "stdout differs\nexpected stdout:\n{}actual stdout:\n{}",
                    indent_lines(expected.iter().map(String::as_str)),
                    indent_lines(actual_lines.iter().copied())
                ));
            }
        }
        StdoutExpectation::Contains(expected) => {
            let missing: Vec<&str> = expected
                .iter()
                .map(String::as_str)
                .filter(|line| !actual_lines.contains(line))
                .collect();
            if !missing.is_empty() {
                problems.push(format!(
                    "stdout is missing expected line(s)\nmissing:\n{}actual stdout:\n{}",
                    indent_lines(missing.iter().copied()),
                    indent_lines(actual_lines.iter().copied())
                ));
            }
        }
    }

    if problems.is_empty() {
        return Ok(());
    }
    let mut detail = problems.join("\n");
    detail.push_str("\nstderr:\n");
    detail.push_str(&indent_block(&String::from_utf8_lossy(&run.stderr)));
    Err(detail)
}

/// `0`, `1`, or `signal` when the process was killed rather than exited.
fn describe_status(output: &Output) -> String {
    output
        .status
        .code()
        .map_or_else(|| format!("no exit code ({})", output.status), |code| code.to_string())
}

/// Render codes as a comma-separated list for a report line.
fn list_codes(codes: &[String]) -> String {
    codes.join(", ")
}

/// Indent every line by two spaces; an empty input renders as `(empty)` so a blank block is visibly blank.
fn indent_lines<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    if out.is_empty() {
        out.push_str("  (empty)\n");
    }
    out
}

/// [`indent_lines`] over a block of text.
fn indent_block(text: &str) -> String {
    indent_lines(text.lines())
}

/// Run every fixture in an area and collect the report; a fixture that cannot be read or run at all is an error of
/// the harness, not a failure of the fixture, and comes back as `Err`.
pub fn run_area(area: &str) -> Result<AreaReport, Box<dyn Error>> {
    let dir = area_dir(area);
    let fixtures = discover(&dir)?;
    let scratch = scratch_root()?;
    let mut report = AreaReport {
        area: checkout_relative(&dir),
        passed: Vec::new(),
        failures: Vec::new(),
    };
    for fixture in &fixtures {
        match run_fixture(fixture, &scratch)? {
            Outcome::Passed => report.passed.push(fixture.display_path()),
            Outcome::Failed(failure) => report.failures.push(failure),
        }
    }
    Ok(report)
}

/// The one call a behaviour root makes: run the area and fail with every failing fixture's expected-versus-actual.
///
/// A harness error (an unreadable fixture, a scratch directory that cannot be created) comes back as `Err`; a fixture
/// that ran and did not show what it declared is an assertion failure, so libtest prints the report as written
/// rather than as one escaped string.
pub fn assert_area_green(area: &str) -> Result<(), Box<dyn Error>> {
    let report = run_area(area)?;
    assert!(report.failures.is_empty(), "{}", report.message());
    Ok(())
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    /// A run fixture: exact stdout, explicit exit code, two retires.
    #[test]
    fn header_with_exact_stdout_and_exit_code() -> TestResult {
        let header = parse_header(
            "# behavior: prints a greeting\n\
             # retires: loaves/compiler/incan_emit/src/codegen.rs::test_fstring_generation\n\
             # retires: loaves/compiler/incan_emit/src/codegen.rs::test_simple_function\n\
             # expect-stdout:\n\
             #   Hello, Ada!\n\
             #\n\
             #     indented line\n\
             # expect-exit: 3\n\
             \n\
             def main() -> None:\n  pass\n",
        )?;
        assert_eq!(header.behavior, "prints a greeting");
        assert_eq!(header.retires.len(), 2);
        assert_eq!(
            header.expectation,
            Expectation::Run {
                stdout: StdoutExpectation::Exact(vec![
                    "Hello, Ada!".to_string(),
                    String::new(),
                    "  indented line".to_string(),
                ]),
                exit_code: 3,
            }
        );
        Ok(())
    }

    /// `expect-stdout-contains:` lines and the default exit code.
    #[test]
    fn header_with_contained_lines_defaults_to_exit_zero() -> TestResult {
        let header = parse_header("# behavior: b\n# expect-stdout-contains:\n#   one\n#   two\n")?;
        assert_eq!(
            header.expectation,
            Expectation::Run {
                stdout: StdoutExpectation::Contains(vec!["one".to_string(), "two".to_string()]),
                exit_code: 0,
            }
        );
        Ok(())
    }

    /// A bare `#` between directives separates; the header ends at the first non-comment line.
    #[test]
    fn separators_and_end_of_header() -> TestResult {
        let header = parse_header("# behavior: b\n#\n# expect-exit: 0\n\n# expect-exit: 9\n")?;
        assert_eq!(
            header.expectation,
            Expectation::Run {
                stdout: StdoutExpectation::Unchecked,
                exit_code: 0,
            }
        );
        Ok(())
    }

    /// A refused program declares diagnostics and nothing about a run.
    #[test]
    fn header_for_a_refused_program() -> TestResult {
        let header =
            parse_header("# behavior: refused\n# expect-diagnostic: INCAN-T0001\n# expect-diagnostic: INCAN-T0002\n")?;
        assert_eq!(
            header.expectation,
            Expectation::Refused {
                diagnostics: vec!["INCAN-T0001".to_string(), "INCAN-T0002".to_string()],
            }
        );
        Ok(())
    }

    /// Every malformed header is refused with a reason that names the problem.
    #[test]
    fn malformed_headers_are_refused_with_the_reason() {
        let cases: &[(&str, &str)] = &[
            ("# expect-exit: 0\n", "no `behavior:`"),
            ("# behavior: b\n", "declares no observable"),
            ("# behavior: b\n# behavior: c\n# expect-exit: 0\n", "declared twice"),
            ("# behavior: b\n# expects-stdout:\n#   x\n", "unknown directive"),
            ("# behavior: b\n# just prose\n# expect-exit: 0\n", "neither a directive"),
            ("# behavior: b\n#   stray\n# expect-exit: 0\n", "outside an"),
            ("# behavior: b\n# expect-stdout: inline\n", "takes its lines below"),
            (
                "# behavior: b\n# expect-stdout:\n#   a\n# expect-stdout-contains:\n#   a\n",
                "cannot both",
            ),
            ("# behavior: b\n# expect-exit: zero\n", "must be an integer"),
            (
                "# behavior: b\n# expect-diagnostic: INCAN-T0001\n# expect-exit: 1\n",
                "never run",
            ),
            (
                "# behavior: b\n# retires: not-a-test\n# expect-exit: 0\n",
                "`<path>.rs::<fn>`",
            ),
            (
                "# behavior: b\n# retires: a.rs::t\n# retires: a.rs::t\n# expect-exit: 0\n",
                "declared twice",
            ),
            ("# behavior: b\n# expect-diagnostic:\n", "one diagnostic code"),
        ];
        for (text, expected) in cases {
            match parse_header(text) {
                Ok(header) => panic!("expected `{text}` to be refused, got {header:?}"),
                Err(reason) => assert!(
                    reason.contains(expected),
                    "expected the reason for `{text}` to mention `{expected}`, got `{reason}`"
                ),
            }
        }
    }

    /// The five smoke fixtures in the tree parse, each retires at least one test, and each declares a run.
    #[test]
    fn smoke_area_fixtures_parse() -> TestResult {
        let fixtures = discover(&area_dir("smoke"))?;
        assert!(
            fixtures.len() >= 5,
            "expected at least five smoke fixtures, found {}",
            fixtures.len()
        );
        for fixture in &fixtures {
            assert!(
                !fixture.header.retires.is_empty(),
                "{} retires nothing; a smoke fixture is a twin",
                fixture.display_path()
            );
        }
        Ok(())
    }

    /// Discovery refuses a stray file and an empty area, and recognises the three layouts.
    #[test]
    fn discovery_recognises_layouts_and_refuses_strays() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let area = tmp.path().join("area");
        fs::create_dir_all(&area)?;
        assert!(discover(&area).is_err(), "an empty area must be refused");

        fs::write(area.join("single.incn"), "# behavior: b\n# expect-exit: 0\n")?;
        fs::create_dir_all(area.join("modules"))?;
        fs::write(area.join("modules/main.incn"), "# behavior: b\n# expect-exit: 0\n")?;
        fs::write(area.join("modules/helper.incn"), "pub def f() -> int:\n  return 1\n")?;
        fs::create_dir_all(area.join("project/src"))?;
        fs::write(area.join("project/loaf.toml"), "[project]\nname = \"p\"\n")?;
        fs::write(area.join("project/src/main.incn"), "# behavior: b\n# expect-exit: 0\n")?;
        fs::write(area.join(".hidden"), "")?;
        let fixtures = discover(&area)?;
        assert_eq!(
            fixtures.iter().map(|f| (f.name.as_str(), f.layout)).collect::<Vec<_>>(),
            vec![
                ("modules", FixtureLayout::Modules),
                ("project", FixtureLayout::Project),
                ("single", FixtureLayout::SingleFile),
            ]
        );

        fs::write(area.join("notes.txt"), "stray")?;
        let error = discover(&area).err().ok_or("a stray file must be refused")?;
        assert!(error.reason.contains("not a fixture"), "{error}");
        Ok(())
    }

    /// Materializing each layout yields a project whose entrypoint carries the header.
    #[test]
    fn materialize_each_layout() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let area = tmp.path().join("area");
        fs::create_dir_all(area.join("modules"))?;
        fs::write(area.join("modules/main.incn"), "# behavior: b\n# expect-exit: 0\n")?;
        fs::write(area.join("modules/helper.incn"), "pub def f() -> int:\n  return 1\n")?;
        fs::create_dir_all(area.join("project/src"))?;
        fs::write(area.join("project/loaf.toml"), "[project]\nname = \"p\"\n")?;
        fs::write(area.join("project/src/main.incn"), "# behavior: b\n# expect-exit: 0\n")?;
        fs::write(area.join("single.incn"), "# behavior: b\n# expect-exit: 0\n")?;
        for fixture in discover(&area)? {
            let project = tmp.path().join(format!("out-{}", fixture.name));
            materialize(&fixture, &project)?;
            let main = fs::read_to_string(project.join(ENTRYPOINT))?;
            assert!(main.starts_with("# behavior: b"), "{}: {main}", fixture.name);
            assert!(project.join("loaf.toml").is_file(), "{}: no manifest", fixture.name);
            if fixture.layout == FixtureLayout::Modules {
                assert!(project.join("src/helper.incn").is_file());
            }
        }
        Ok(())
    }

    /// A report names every failing fixture, its behaviour and the mismatch, and lists what passed.
    #[test]
    fn report_message_lists_every_failure() {
        let report = AreaReport {
            area: "loaves/x/behavior/smoke".to_string(),
            passed: vec!["loaves/x/behavior/smoke/ok.incn".to_string()],
            failures: vec![
                FixtureFailure {
                    path: "loaves/x/behavior/smoke/a.incn".to_string(),
                    behavior: "a".to_string(),
                    detail: "stdout differs\n".to_string(),
                },
                FixtureFailure {
                    path: "loaves/x/behavior/smoke/b.incn".to_string(),
                    behavior: "b".to_string(),
                    detail: "exit code: expected 0, got 1\n".to_string(),
                },
            ],
        };
        let message = report.message();
        assert!(message.starts_with("2 of 3 behaviour fixture(s) failed in loaves/x/behavior/smoke"));
        assert!(message.contains("--- loaves/x/behavior/smoke/a.incn\nbehavior: a\nstdout differs"));
        assert!(message.contains("--- loaves/x/behavior/smoke/b.incn\nbehavior: b\nexit code: expected 0, got 1"));
        assert!(message.contains("passed:\n  loaves/x/behavior/smoke/ok.incn"));
    }

    /// Exact stdout is compared line by line; contained lines in any order; the exit code always.
    #[cfg(unix)]
    #[test]
    fn run_comparison_reports_each_mismatch() {
        let output = |code: i32, stdout: &str| Output {
            status: exit_status(code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: b"trace\n".to_vec(),
        };
        let exact = StdoutExpectation::Exact(vec!["one".to_string(), "two".to_string()]);
        assert_eq!(compare_run(&exact, 0, &output(0, "one\ntwo\n")), Ok(()));
        let differs = compare_run(&exact, 0, &output(0, "one\n")).err().unwrap_or_default();
        assert!(differs.contains("expected stdout:\n  one\n  two\n"), "{differs}");
        assert!(differs.contains("actual stdout:\n  one\n"), "{differs}");
        assert!(differs.contains("stderr:\n  trace"), "{differs}");

        let contains = StdoutExpectation::Contains(vec!["two".to_string()]);
        assert_eq!(compare_run(&contains, 0, &output(0, "one\ntwo\n")), Ok(()));
        let missing = compare_run(&contains, 0, &output(0, "one\n")).err().unwrap_or_default();
        assert!(missing.contains("missing:\n  two\n"), "{missing}");

        let code = compare_run(&StdoutExpectation::Unchecked, 2, &output(1, ""))
            .err()
            .unwrap_or_default();
        assert!(code.contains("exit code: expected 2, got 1"), "{code}");
    }

    /// A refusal is proved by a failing check whose JSON report carries every declared code.
    #[cfg(unix)]
    #[test]
    fn refusal_comparison_reads_the_json_report() {
        let output = |code: i32, stdout: &str| Output {
            status: exit_status(code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        };
        let expected = vec!["INCAN-T0001".to_string()];
        let report = r#"{"schema_version":2,"ok":false,"diagnostics":[{"code":"INCAN-T0001","severity":"error"}]}"#;
        assert_eq!(compare_refusal(&expected, &output(1, report)), Ok(()));
        let accepted = compare_refusal(&expected, &output(0, r#"{"ok":true,"diagnostics":[]}"#))
            .err()
            .unwrap_or_default();
        assert!(accepted.contains("but it accepted it"), "{accepted}");
        let other = r#"{"ok":false,"diagnostics":[{"code":"INCAN-T0002"}]}"#;
        let wrong = compare_refusal(&expected, &output(1, other)).err().unwrap_or_default();
        assert!(wrong.contains("missing: INCAN-T0001"), "{wrong}");
        assert!(wrong.contains("reported: INCAN-T0002"), "{wrong}");
    }

    /// Build an exit status for a comparison test without spawning anything (the compiler suite is Unix-only, and
    /// so is the wait-status encoding this relies on).
    #[cfg(unix)]
    fn exit_status(code: i32) -> std::process::ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code << 8)
    }
}
