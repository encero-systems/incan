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
//! `INCAN_TEST_TMP_ROOT`, bakes the in-fixture providers a project fixture declares (its `loaf.toml` path
//! dependencies, in dependency order, with no Cargo authority: a provider whose bake needs Cargo fails under the
//! suite's guard, attributed to the fixture), runs it, compares the observables and reports **every** failing
//! fixture with its path and its expected-versus-actual, so one libtest case per area still attributes a failure to
//! the fixture that caused it. Roots are thin: `loaves/toolchain/incan-cli/tests/behavior_<area>_tests.rs` calls
//! [`assert_area_green`] and nothing else, and no behaviour root is registered for a suite capability. The inventory
//! (`scripts/test_inventory/collect.py`) reads the same header for its `# retires:` lines, so the header grammar here
//! and the collector's reader must agree.

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use crate::cli_project::{
    run_explicit_oven_bake, run_guarded_oven_bake_with_home, run_incan, standalone_oven_home, write_minimal_project,
};

/// Directory under [`crate::fixtures_dir`] that holds every behaviour-fixture area.
pub const BEHAVIOR_FIXTURES_ROOT: &str = "behavior";

/// The entrypoint every materialized fixture project runs and checks.
const ENTRYPOINT: &str = "src/main.incn";

/// The most fixtures one leaf area may hold.
///
/// An area is one libtest case, and the compiler suite runs roots on two threads with no slicing inside a case
/// (#1549 slices by case), so the case is the unit of budget: at roughly 3.5 s per fixture locally and 6-10 s on the
/// four-core CI runner, 60 fixtures is about 3.5 minutes locally and 6-10 minutes in CI, the most one case should
/// cost. A larger family splits into leaf areas of at most this size, each its own `#[test]` in the root file (one
/// root file may hold several, one per leaf area) and its own `fixture_roots` entry in the inventory; only leaf
/// areas are declared there, because a parent directory of areas would be discovered as module fixtures.
pub const MAX_FIXTURES_PER_AREA: usize = 60;

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

/// One in-fixture provider of a project fixture: a path dependency its `loaf.toml` declares (directly or through
/// another provider) whose project lives under the fixture directory. The runner bakes every provider before the
/// program runs, because a consumer refuses to run until its `pub::` providers have published a package Loaf. The
/// bake carries no Cargo authority: an Incan library with no dependencies of its own is served from the active
/// standard-library Loaf without Cargo, and a provider that needs more (one that itself declares `[dependencies]`)
/// fails its fixture under the compiler suite's Cargo guard rather than being admitted to Cargo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    /// The dependency's name in the manifest that first declared it (`pub::<name>` on the consumer's side).
    pub name: String,
    /// The provider's directory relative to the fixture directory, lexically normalised (`deps/querykit`).
    pub path: PathBuf,
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
    /// The in-fixture providers a project fixture declares, in bake order: a provider comes after the providers it
    /// depends on. Empty for the other layouts and for a project without path dependencies.
    pub providers: Vec<Provider>,
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

/// A stdout block: the line that opened it and the expected lines collected under it.
type Block = (usize, Vec<String>);

/// Parse the leading comment block of a fixture program.
///
/// The header is the unbroken run of lines at the top of the file that start with `#`; the first line that does not,
/// a blank line included, ends it. Every header line is a directive (`# key: value`, one space between `#` and the
/// key), an item of the block directive above it (`#` followed by two or more spaces, or a bare `#` for an empty
/// expected line), or a bare `#` between directives, which is ignored. Anything else is refused with the line number,
/// and so is an unknown directive, a `behavior:` that is missing or repeated, a stdout block beside another stdout
/// block, an `expect-diagnostic:` beside any run expectation, an `expect-stdout-contains:` block with no line (anything
/// would satisfy it), a contained line listed twice (one occurrence satisfies both), an expected line that ends with
/// whitespace (a report cannot show it), an `expect-exit:` outside `0..=255`, and a header that declares no observable
/// at all: a fixture that proves nothing is not a twin. A directive that appears after the header ended is refused
/// too, naming the line: it would otherwise be ignored and the fixture would pass on less than it appears to declare.
pub fn parse_header(text: &str) -> Result<Header, String> {
    let mut behavior: Option<String> = None;
    let mut retires: Vec<String> = Vec::new();
    // Every expectation remembers the line that introduced it, so a refusal that involves two directives names both.
    let mut stdout_exact: Option<Block> = None;
    let mut stdout_contains: Option<Block> = None;
    let mut exit_code: Option<(usize, i32)> = None;
    let mut diagnostics: Vec<String> = Vec::new();
    let mut first_diagnostic: Option<usize> = None;
    let mut first_run: Option<(usize, &str)> = None;
    // The block directive currently collecting items, with the indentation its first item established.
    let mut open_block: Option<(&'static str, Option<usize>)> = None;
    // Where the header ended: the first line that is not a `#` comment, and whether that line was blank.
    let mut header_end: Option<(usize, bool)> = None;

    for (index, line) in text.lines().enumerate() {
        let number = index + 1;

        // ---- Past the header: a directive here would be ignored, so it is refused ----
        if let Some((end, blank)) = header_end {
            if let Some(key) = late_directive(line) {
                return Err(late_directive_reason(number, key, end, blank));
            }
            continue;
        }
        let Some(rest) = line.strip_prefix('#') else {
            header_end = Some((number, line.trim().is_empty()));
            continue;
        };

        // ---- Bare `#`: an empty expected line inside a block, a separator outside one ----
        if rest.trim().is_empty() {
            if let Some((key, _)) = open_block {
                // Only a bare `#` is the empty expected line; `#` and whitespace would be an expected line made of
                // whitespace, which the block-item rule refuses, so it is refused here too rather than coerced.
                if !rest.is_empty() {
                    return Err(format!(
                        "line {number}: inside the `{key}:` block, `#` followed only by whitespace is an expected line that ends with whitespace, which a report cannot show; a bare `#` is the empty expected line"
                    ));
                }
                push_block_item(key, number, String::new(), &mut stdout_exact, &mut stdout_contains)?;
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
            push_block_item(key, number, item, &mut stdout_exact, &mut stdout_contains)?;
            continue;
        }

        // ---- Directive: `# key: value`, one space after `#` ----
        let directive = rest.strip_prefix(' ').unwrap_or(rest);
        if directive.starts_with(char::is_whitespace) {
            return Err(format!(
                "line {number}: a directive is spelled `# key: value`, with one space between `#` and the key"
            ));
        }
        let Some((key, raw_value)) = directive.split_once(':') else {
            return Err(format!(
                "line {number}: `#{rest}` is neither a directive (`# key: value`) nor an indented block item; put prose in `behavior:`"
            ));
        };
        if key != key.trim_end() {
            return Err(format!(
                "line {number}: `{}:` has whitespace before the colon; a directive is spelled `# key: value`",
                key.trim_end()
            ));
        }
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
                let test = retires_key(number, value)?;
                if retires.contains(&test) {
                    return Err(format!("line {number}: `retires: {value}` is declared twice"));
                }
                retires.push(test);
            }
            "expect-stdout" | "expect-stdout-contains" => {
                if !value.is_empty() {
                    return Err(format!(
                        "line {number}: `{key}:` takes its lines below it, indented by two spaces after `#`"
                    ));
                }
                if let Some(line) = first_diagnostic {
                    return Err(run_beside_refusal_reason(number, key, line));
                }
                let (block, slot, other, other_key) = if key == EXPECT_STDOUT {
                    (
                        EXPECT_STDOUT,
                        &mut stdout_exact,
                        &stdout_contains,
                        EXPECT_STDOUT_CONTAINS,
                    )
                } else {
                    (
                        EXPECT_STDOUT_CONTAINS,
                        &mut stdout_contains,
                        &stdout_exact,
                        EXPECT_STDOUT,
                    )
                };
                if let Some((line, _)) = slot {
                    return Err(format!(
                        "line {number}: `{key}:` is declared twice (first on line {line})"
                    ));
                }
                if let Some((line, _)) = other {
                    return Err(format!(
                        "line {number}: `{key}:` cannot be declared beside `{other_key}:` (line {line}); pick one"
                    ));
                }
                *slot = Some((number, Vec::new()));
                first_run.get_or_insert((number, key));
                open_block = Some((block, None));
            }
            "expect-exit" => {
                if let Some((line, _)) = exit_code {
                    return Err(format!(
                        "line {number}: `expect-exit:` is declared twice (first on line {line})"
                    ));
                }
                let code = value
                    .parse::<i32>()
                    .map_err(|_| format!("line {number}: `expect-exit:` must be an integer, got `{value}`"))?;
                if !(0..=255).contains(&code) {
                    return Err(format!(
                        "line {number}: `expect-exit:` must be an exit code in 0..=255, got `{value}`"
                    ));
                }
                if let Some(line) = first_diagnostic {
                    return Err(run_beside_refusal_reason(number, key, line));
                }
                exit_code = Some((number, code));
                first_run.get_or_insert((number, key));
            }
            "expect-diagnostic" => {
                if value.is_empty() || value.contains(char::is_whitespace) {
                    return Err(format!(
                        "line {number}: `expect-diagnostic:` must name one diagnostic code such as `INCAN-T0001`"
                    ));
                }
                if let Some((line, run_key)) = first_run {
                    return Err(format!(
                        "line {number}: `expect-diagnostic:` means the program is refused at check time and never run, but line {line} declares `{run_key}:`, a run expectation; drop one side"
                    ));
                }
                if diagnostics.iter().any(|existing| existing == value) {
                    return Err(format!("line {number}: `expect-diagnostic: {value}` is declared twice"));
                }
                first_diagnostic.get_or_insert(number);
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
    if let Some((line, items)) = &stdout_contains
        && items.is_empty()
    {
        return Err(format!(
            "line {line}: `expect-stdout-contains:` lists no line, so anything would satisfy it; name at least one line, or use an empty `expect-stdout:` for a program that prints nothing"
        ));
    }
    if !diagnostics.is_empty() {
        return Ok(Header {
            behavior,
            retires,
            expectation: Expectation::Refused { diagnostics },
        });
    }
    if first_run.is_none() {
        return Err(
            "the header declares no observable: add `expect-stdout:`, `expect-stdout-contains:`, `expect-exit:` or `expect-diagnostic:`"
                .to_string(),
        );
    }
    let stdout = match (stdout_exact, stdout_contains) {
        (Some((_, lines)), _) => StdoutExpectation::Exact(lines),
        (None, Some((_, lines))) => StdoutExpectation::Contains(lines),
        (None, None) => StdoutExpectation::Unchecked,
    };
    Ok(Header {
        behavior,
        retires,
        expectation: Expectation::Run {
            stdout,
            exit_code: exit_code.map_or(0, |(_, code)| code),
        },
    })
}

/// Validate a `retires:` value as the inventory keys a test: `<path>.rs::<fn>`, no whitespace anywhere, the function
/// part module-qualified when the inventory needs it (`tests::inner::name`). The collector
/// (`scripts/test_inventory/collect.py`) reads the same line with the same rule, so a key both readers accept is one
/// the inventory can resolve, and a key one refuses the other never silently reads differently.
fn retires_key(number: usize, value: &str) -> Result<String, String> {
    let refuse =
        || format!("line {number}: `retires:` must name a test as `<path>.rs::<fn>` with no whitespace, got `{value}`");
    if value.contains(char::is_whitespace) {
        return Err(refuse());
    }
    let Some((path, test)) = value.split_once("::") else {
        return Err(refuse());
    };
    let Some(stem) = path.strip_suffix(".rs") else {
        return Err(refuse());
    };
    if stem.is_empty() || test.is_empty() {
        return Err(refuse());
    }
    Ok(value.to_string())
}

/// The refusal for a run expectation declared after an `expect-diagnostic:`, naming both lines.
fn run_beside_refusal_reason(number: usize, key: &str, diagnostic_line: usize) -> String {
    format!(
        "line {number}: `{key}:` is a run expectation, but line {diagnostic_line} declares `expect-diagnostic:`, which means the program is refused at check time and never run; drop one side"
    )
}

/// The directive a line past the header spells, when it spells one: `#`, optional whitespace, a known key (or any
/// `expect-` key, so a misspelt expectation is caught too) and a colon.
fn late_directive(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('#')?.trim_start();
    let (key, _) = rest.split_once(':')?;
    let known = key == "behavior"
        || key == "retires"
        || key
            .strip_prefix("expect-")
            .is_some_and(|tail| !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'));
    known.then_some(key)
}

/// The refusal for a directive past the header: it names the offending line and the line that ended the header,
/// because a blank line inside what the author meant as one header is the usual cause.
fn late_directive_reason(number: usize, key: &str, end: usize, blank: bool) -> String {
    let ended_by = if blank {
        format!("a blank line ends the header (line {end})")
    } else {
        format!("the first line that is not a `#` comment ends the header (line {end})")
    };
    format!(
        "line {number}: `{key}:` after the header ended; {ended_by}, so keep every directive in the unbroken run of `#` lines at the top of the file"
    )
}

/// Append one expected line to whichever stdout block is open, refusing what the comparison could not report
/// faithfully: a line that ends with whitespace (invisible in the report, so a mismatch there would be unreadable)
/// and, in a contains block, a line listed twice (one occurrence of it satisfies both entries).
fn push_block_item(
    key: &str,
    number: usize,
    item: String,
    stdout_exact: &mut Option<Block>,
    stdout_contains: &mut Option<Block>,
) -> Result<(), String> {
    if item != item.trim_end() {
        return Err(format!(
            "line {number}: the expected line `{item}` ends with whitespace, which a report cannot show; remove it"
        ));
    }
    let contains = key == EXPECT_STDOUT_CONTAINS;
    let slot = if contains { stdout_contains } else { stdout_exact };
    let Some((_, lines)) = slot.as_mut() else {
        return Ok(());
    };
    if contains && lines.contains(&item) {
        return Err(format!(
            "line {number}: `expect-stdout-contains:` lists `{item}` twice; one occurrence of the line satisfies both, so the repeat checks nothing"
        ));
    }
    lines.push(item);
    Ok(())
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
/// would otherwise silently prove nothing. An area with no fixture at all is refused for the same reason, and so is
/// an area with more than [`MAX_FIXTURES_PER_AREA`] fixtures: the area is one libtest case, and a case that large
/// no longer fits the suite's per-root budget.
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
    if fixtures.len() > MAX_FIXTURES_PER_AREA {
        return Err(FixtureFormatError {
            path: area_display,
            reason: format!(
                "the area holds {} fixtures; a leaf area holds at most {MAX_FIXTURES_PER_AREA} (one libtest case, about 3.5 minutes locally and 6-10 minutes on the CI runner). Split it into leaf areas of at most {MAX_FIXTURES_PER_AREA} fixtures, one `#[test]` per area in the root file calling `assert_area_green`, each leaf declared in `fixture_roots`",
                fixtures.len()
            ),
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
    let providers = match layout {
        FixtureLayout::Project => provider_plan(path).map_err(refuse)?,
        FixtureLayout::SingleFile | FixtureLayout::Modules => Vec::new(),
    };
    Ok(BehaviorFixture {
        path: path.to_path_buf(),
        name,
        layout,
        header,
        providers,
    })
}

// ============================================================
// Providers
// ============================================================

/// The in-fixture providers of a project fixture, in bake order.
///
/// The fixture's `loaf.toml` is read the way the compiler reads it (`oven_model::manifest::ProjectManifest`), and
/// every `[dependencies]` path entry is followed: the provider's own manifest is read in turn, so a provider that
/// depends on another provider is planned after it. A dependency whose resolved path leaves the fixture directory
/// is refused (the fixture would depend on something the runner does not copy), and so is a cycle, named as the
/// chain that closes it; a fixture manifest the compiler cannot read is refused with the compiler's own words. A
/// *provider's* manifest that cannot be read is not refused here: the provider is planned with no dependencies of
/// its own and its bake reports what is wrong, attributed to the fixture like every other bake failure, so one
/// broken provider fails one fixture rather than the whole area.
fn provider_plan(fixture_dir: &Path) -> Result<Vec<Provider>, String> {
    let mut plan = Vec::new();
    let mut visiting = Vec::new();
    plan_providers_of(fixture_dir, Path::new(""), &mut visiting, &mut plan)?;
    Ok(plan)
}

/// Plan the providers declared by the manifest at `fixture_dir/relative` (the fixture itself when `relative` is
/// empty), depth first, so each provider lands in `plan` after the providers it depends on. `visiting` is the chain
/// of provider directories currently being followed, which is how a cycle is detected and named.
fn plan_providers_of(
    fixture_dir: &Path,
    relative: &Path,
    visiting: &mut Vec<PathBuf>,
    plan: &mut Vec<Provider>,
) -> Result<(), String> {
    let manifest_path = fixture_dir.join(relative).join("loaf.toml");
    let manifest = match oven_model::manifest::ProjectManifest::load(&manifest_path) {
        Ok(manifest) => manifest,
        Err(error) if relative.as_os_str().is_empty() => {
            return Err(format!("cannot read the fixture's `loaf.toml`: {error}"));
        }
        // A provider the compiler refuses: its bake says why, attributed to the fixture.
        Err(_) => return Ok(()),
    };
    let mut dependencies: Vec<_> = manifest.library_dependencies().values().collect();
    dependencies.sort_by(|left, right| left.library_name.cmp(&right.library_name));
    for dependency in dependencies {
        let resolved = normalize_lexically(&dependency.path);
        let Ok(provider_relative) = resolved.strip_prefix(fixture_dir) else {
            return Err(format!(
                "`{}` in `{}` resolves to `{}`, outside the fixture directory; an in-fixture provider lives under the fixture (`deps/{}`), because only the fixture directory is copied into the scratch project",
                dependency.library_name,
                display_relative(manifest_path.strip_prefix(fixture_dir).unwrap_or(&manifest_path)),
                resolved.display(),
                dependency.library_name
            ));
        };
        let provider_relative = provider_relative.to_path_buf();
        if provider_relative.as_os_str().is_empty() || visiting.contains(&provider_relative) {
            let mut chain: Vec<String> = std::iter::once(String::from("<fixture>"))
                .chain(visiting.iter().map(|directory| display_relative(directory)))
                .collect();
            chain.push(if provider_relative.as_os_str().is_empty() {
                String::from("<fixture>")
            } else {
                display_relative(&provider_relative)
            });
            return Err(format!(
                "the path dependencies form a cycle: {}; a provider cannot depend on a project that depends on it, because neither could be baked first",
                chain.join(" -> ")
            ));
        }
        if plan.iter().any(|provider| provider.path == provider_relative) {
            continue;
        }
        visiting.push(provider_relative.clone());
        plan_providers_of(fixture_dir, &provider_relative, visiting, plan)?;
        visiting.pop();
        plan.push(Provider {
            name: dependency.library_name.clone(),
            path: provider_relative,
        });
    }
    Ok(())
}

/// Resolve `.` and `..` components without touching the file system, so a provider path such as `../money`
/// declared by another provider is compared with the fixture directory as the path it names.
fn normalize_lexically(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component);
                }
            }
            other => normalized.push(other),
        }
    }
    normalized
}

/// A fixture-relative path with `/` separators, for messages.
fn display_relative(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
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
/// process temporary directory. An explicit root that does not exist is an error, not created and not a fallback
/// (the rule `CONTRIBUTING.md` states for the suite's fixture scratch): a root that silently appears somewhere
/// unexpected is how scratch escapes the managed directory.
pub fn scratch_root() -> Result<PathBuf, Box<dyn Error>> {
    scratch_root_from(std::env::var_os("INCAN_TEST_TMP_ROOT").map(PathBuf::from))
}

/// [`scratch_root`] over an already-read `INCAN_TEST_TMP_ROOT` value (`None` or empty = unset), so the rule is
/// testable without touching the process environment.
fn scratch_root_from(explicit: Option<PathBuf>) -> Result<PathBuf, Box<dyn Error>> {
    let Some(root) = explicit.filter(|value| !value.as_os_str().is_empty()) else {
        return Ok(std::env::temp_dir());
    };
    if !root.is_dir() {
        return Err(format!(
            "INCAN_TEST_TMP_ROOT names `{}`, which is not an existing directory; create it first (an invalid explicit root is an error, not a fallback)",
            root.display()
        )
        .into());
    }
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
/// A refused-program fixture goes through `incan check --format json` only; its providers are not baked, because
/// `incan check` prepares an unbaked provider's metadata itself. A run fixture first has every in-fixture provider
/// baked, in the order the plan lists them (a consumer refuses to run until its `pub::` providers have published a
/// package Loaf), with no Cargo authority: outside the compiler suite the bake publishes into the fixture project's
/// own standalone Oven home (a provider baked into its own home would be invisible to the consumer's run), and under
/// the suite any Cargo the bake reaches for is the scheduler's guard, so a provider that needs Cargo fails its
/// fixture loudly. The project itself is then baked outside the suite only (a fresh project has no Loaf authority
/// until then; under the suite the sealed standard-library Loaf serves), and run through `incan run`, which is the
/// legacy route today and whatever slice 7 makes it tomorrow. A bake that fails is a failure of the fixture, with
/// the bake's output, like any other mismatch. The scratch project is deleted when this returns, pass or fail, so
/// the stderr the report carries is all a failure leaves behind.
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
            // ---- Providers first, in dependency order, Cargo-guarded, into the project's own Oven home ----
            let home = standalone_oven_home(project.path());
            for provider in &fixture.providers {
                let what = format!(
                    "the provider `{}` (`{}`)",
                    provider.name,
                    display_relative(&provider.path)
                );
                let bake = run_guarded_oven_bake_with_home(&project.path().join(&provider.path), Some(&home))?;
                if let Err(detail) = bake_result(&what, &bake) {
                    return Ok(outcome(Err(detail)));
                }
            }
            // ---- The project itself, outside the suite only ----
            if !crate::oven_compiler_suite_is_active() {
                let bake = run_explicit_oven_bake(project.path())?;
                if let Err(detail) = bake_result("the project", &bake) {
                    return Ok(outcome(Err(detail)));
                }
            }
            let run = run_incan(project.path(), &["run", ENTRYPOINT])?;
            Ok(outcome(compare_run(stdout, *exit_code, &run)))
        }
    }
}

/// Lay out a failed bake for the fixture's failure report; a successful one is `Ok`.
fn bake_result(what: &str, bake: &Output) -> Result<(), String> {
    if bake.status.success() {
        return Ok(());
    }
    Err(format!(
        "baking {what} failed (exit {})\nstdout:\n{}stderr:\n{}",
        describe_status(bake),
        indent_block(&String::from_utf8_lossy(&bake.stdout)),
        indent_block(&String::from_utf8_lossy(&bake.stderr))
    ))
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

/// Run every fixture in an area and collect the report; a fixture that cannot be read or run at all, or an area
/// over [`MAX_FIXTURES_PER_AREA`], is an error of the harness, not a failure of the fixture, and comes back as `Err`.
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
/// A harness error (an unreadable fixture, a missing scratch root, an area over [`MAX_FIXTURES_PER_AREA`]) comes
/// back as `Err`; a fixture that ran and did not show what it declared is an assertion failure, so libtest prints
/// the report as written rather than as one escaped string. A root file holds one `#[test]` per leaf area, each
/// making this call, so the suite's two-thread root budget can run two areas of one family concurrently.
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

    /// A bare `#` between directives separates, and so does `#` with only whitespace outside a block; the header
    /// ends at the first non-comment line, after which ordinary comments (indented ones included) and a
    /// `key: value` that is not a directive are the program's business.
    #[test]
    fn separators_and_end_of_header() -> TestResult {
        let header = parse_header(
            "# behavior: b\n#\n#   \n# expect-exit: 0\n\n# note: a comment, not a directive\ndef main() -> None:\n    # expect-exit: 9 would be a directive at column 0\n    pass\n",
        )?;
        assert_eq!(
            header.expectation,
            Expectation::Run {
                stdout: StdoutExpectation::Unchecked,
                exit_code: 0,
            }
        );
        Ok(())
    }

    /// An empty `expect-stdout:` block is a legal observable: the program prints nothing.
    #[test]
    fn empty_exact_block_means_prints_nothing() -> TestResult {
        let header = parse_header("# behavior: b\n# expect-stdout:\n\ndef main() -> None:\n    pass\n")?;
        assert_eq!(
            header.expectation,
            Expectation::Run {
                stdout: StdoutExpectation::Exact(Vec::new()),
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

    /// Every malformed header is refused with a reason that names the problem and, where one exists, the line.
    #[test]
    fn malformed_headers_are_refused_with_the_reason() -> TestResult {
        let cases: &[(&str, &[&str])] = &[
            ("# expect-exit: 0\n", &["no `behavior:`"]),
            ("# behavior: b\n", &["declares no observable"]),
            (
                "# behavior: b\n# behavior: c\n# expect-exit: 0\n",
                &["line 2", "declared twice"],
            ),
            (
                "# behavior: b\n# expects-stdout:\n#   x\n",
                &["line 2", "unknown directive"],
            ),
            (
                "# behavior: b\n# just prose\n# expect-exit: 0\n",
                &["line 2", "neither a directive"],
            ),
            (
                "# behavior: b\n#   stray\n# expect-exit: 0\n",
                &["line 2", "outside an"],
            ),
            (
                "# behavior: b\n# expect-stdout: inline\n",
                &["line 2", "takes its lines below"],
            ),
            // ---- The two stdout blocks: the second one introduces the conflict ----
            (
                "# behavior: b\n# expect-stdout:\n#   a\n# expect-stdout-contains:\n#   a\n",
                &[
                    "line 4",
                    "`expect-stdout-contains:` cannot be declared beside `expect-stdout:` (line 2)",
                ],
            ),
            (
                "# behavior: b\n# expect-stdout-contains:\n#   a\n# expect-stdout:\n#   a\n",
                &[
                    "line 4",
                    "`expect-stdout:` cannot be declared beside `expect-stdout-contains:` (line 2)",
                ],
            ),
            (
                "# behavior: b\n# expect-exit: zero\n",
                &["line 2", "must be an integer"],
            ),
            ("# behavior: b\n# expect-exit: 256\n", &["line 2", "0..=255"]),
            ("# behavior: b\n# expect-exit: -1\n", &["line 2", "0..=255"]),
            // ---- A refusal beside a run expectation: whichever comes second introduces the conflict ----
            (
                "# behavior: b\n# expect-diagnostic: INCAN-T0001\n# expect-exit: 1\n",
                &[
                    "line 3",
                    "`expect-exit:` is a run expectation, but line 2 declares `expect-diagnostic:`",
                ],
            ),
            (
                "# behavior: b\n# expect-stdout:\n#   a\n# expect-diagnostic: INCAN-T0001\n",
                &["line 4", "never run, but line 2 declares `expect-stdout:`"],
            ),
            (
                "# behavior: b\n# retires: not-a-test\n# expect-exit: 0\n",
                &["line 2", "`<path>.rs::<fn>`"],
            ),
            (
                "# behavior: b\n# retires: a b.rs::t\n# expect-exit: 0\n",
                &["line 2", "no whitespace"],
            ),
            (
                "# behavior: b\n# retires: a.rs::t\n# retires: a.rs::t\n# expect-exit: 0\n",
                &["line 3", "declared twice"],
            ),
            (
                "# behavior: b\n# expect-diagnostic:\n",
                &["line 2", "one diagnostic code"],
            ),
            // ---- Directive spelling both readers agree on: one space after `#`, none before the colon ----
            (
                "# behavior: b\n#\tretires: a.rs::t\n# expect-exit: 0\n",
                &["line 2", "one space between"],
            ),
            (
                "# behavior: b\n# retires : a.rs::t\n# expect-exit: 0\n",
                &["line 2", "before the colon"],
            ),
            // ---- B1: a directive after the header ended is refused, not dropped ----
            (
                "# behavior: b\n# expect-exit: 0\n\n# expect-stdout:\n#   x\n\ndef main() -> None:\n    pass\n",
                &[
                    "line 4",
                    "`expect-stdout:` after the header ended; a blank line ends the header (line 3)",
                ],
            ),
            (
                "# behavior: b\n# expect-exit: 0\ndef main() -> None:\n    pass\n# retires: a.rs::t\n",
                &[
                    "line 5",
                    "`retires:` after the header ended; the first line that is not a `#` comment ends the header (line 3)",
                ],
            ),
            (
                "# behavior: b\n# expect-exit: 0\n\n# expect-stdou:\n",
                &["line 4", "`expect-stdou:` after the header ended"],
            ),
            // ---- B1: a contains block with no line would be satisfied by anything ----
            (
                "# behavior: b\n# expect-stdout-contains:\n",
                &["line 2", "lists no line"],
            ),
            (
                "# behavior: b\n# expect-stdout-contains:\n# expect-exit: 0\n",
                &["line 2", "lists no line"],
            ),
            // ---- A contained line listed twice checks nothing the first did not ----
            (
                "# behavior: b\n# expect-stdout-contains:\n#   a\n#   b\n#   a\n",
                &["line 5", "lists `a` twice"],
            ),
            // ---- Trailing whitespace on an expected line is invisible in a report ----
            (
                "# behavior: b\n# expect-stdout:\n#   a \n",
                &["line 3", "ends with whitespace"],
            ),
            (
                "# behavior: b\n# expect-stdout-contains:\n#   a\n#   b\t\n",
                &["line 4", "ends with whitespace"],
            ),
            // ---- `#` and only whitespace inside a block is not the empty expected line ----
            (
                "# behavior: b\n# expect-stdout:\n#   a\n#   \n",
                &[
                    "line 4",
                    "ends with whitespace",
                    "a bare `#` is the empty expected line",
                ],
            ),
            (
                "# behavior: b\n# expect-stdout-contains:\n#      \n#   a\n",
                &["line 3", "ends with whitespace"],
            ),
        ];
        for (text, expected) in cases {
            let reason = parse_header(text)
                .err()
                .ok_or_else(|| format!("expected `{text}` to be refused"))?;
            for fragment in *expected {
                assert!(
                    reason.contains(fragment),
                    "expected the reason for `{text}` to mention `{fragment}`, got `{reason}`"
                );
            }
        }
        Ok(())
    }

    /// Past the header, a comment that mentions a directive name in prose or a `key:` that is no directive is the
    /// program's business; only a directive at the start of a `#` line is refused there.
    #[test]
    fn header_line_after_the_header_ended_is_refused_only_when_it_is_a_directive() -> TestResult {
        let header = parse_header(
            "# behavior: b\n# expect-exit: 0\n\n# This comment mentions expect-stdout: in prose, after a word\n# TODO: not a directive\n#   an indented comment, not a block item\n",
        )?;
        assert_eq!(header.behavior, "b");
        Ok(())
    }

    /// An area is one libtest case, so it is capped: 60 fixtures pass discovery, 61 are refused naming the rule.
    #[test]
    fn discovery_refuses_an_area_over_the_cap() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let area = tmp.path().join("area");
        fs::create_dir_all(&area)?;
        for index in 0..MAX_FIXTURES_PER_AREA {
            fs::write(
                area.join(format!("f{index:03}.incn")),
                "# behavior: b\n# expect-exit: 0\n",
            )?;
        }
        assert_eq!(discover(&area)?.len(), MAX_FIXTURES_PER_AREA);
        fs::write(area.join("one_too_many.incn"), "# behavior: b\n# expect-exit: 0\n")?;
        let error = discover(&area).err().ok_or("an area over the cap must be refused")?;
        assert!(error.reason.contains("holds 61 fixtures"), "{error}");
        assert!(error.reason.contains("at most 60"), "{error}");
        assert!(error.reason.contains("one `#[test]` per area"), "{error}");
        Ok(())
    }

    /// An explicit scratch root that does not exist is refused rather than created; an unset or empty one falls
    /// back to the process temporary directory.
    #[test]
    fn scratch_root_requires_an_explicit_root_to_exist() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let missing = tmp.path().join("missing");
        let refused = scratch_root_from(Some(missing.clone()))
            .err()
            .ok_or("a missing explicit root must be refused")?
            .to_string();
        assert!(refused.contains("not an existing directory"), "{refused}");
        assert!(!missing.exists(), "the missing root must not be created");
        assert_eq!(scratch_root_from(Some(tmp.path().to_path_buf()))?, tmp.path());
        assert_eq!(scratch_root_from(None)?, std::env::temp_dir());
        assert_eq!(scratch_root_from(Some(PathBuf::new()))?, std::env::temp_dir());
        Ok(())
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

    /// Write a project (a `loaf.toml` naming `name` with the given `[dependencies]` entries, and an empty
    /// `src/lib.incn`) at `root`, for the provider-plan tests.
    fn write_project(root: &Path, name: &str, dependencies: &[(&str, &str)]) -> TestResult {
        fs::create_dir_all(root.join("src"))?;
        let mut manifest = format!("[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n");
        if !dependencies.is_empty() {
            manifest.push_str("\n[dependencies]\n");
            for (dependency, path) in dependencies {
                manifest.push_str(&format!("{dependency} = {{ path = \"{path}\" }}\n"));
            }
        }
        fs::write(root.join("loaf.toml"), manifest)?;
        fs::write(root.join("src/lib.incn"), "")?;
        Ok(())
    }

    /// A project fixture's providers are planned in bake order: a provider's own provider (declared relative to the
    /// provider, `../money`) comes first, a provider two consumers share is planned once, and the fixture's direct
    /// dependencies are visited in name order. Single-file and module fixtures plan nothing.
    #[test]
    fn provider_plan_orders_providers_before_their_dependents() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let area = tmp.path().join("area");
        let app = area.join("app");
        write_project(&app, "app", &[("ledger", "deps/ledger"), ("audit", "deps/audit")])?;
        fs::write(app.join("src/main.incn"), "# behavior: b\n# expect-exit: 0\n")?;
        write_project(&app.join("deps/ledger"), "ledger", &[("money", "../money")])?;
        write_project(&app.join("deps/audit"), "audit", &[("money", "./../money/")])?;
        write_project(&app.join("deps/money"), "money", &[])?;
        fs::write(area.join("single.incn"), "# behavior: b\n# expect-exit: 0\n")?;

        let fixtures = discover(&area)?;
        let app = fixtures.iter().find(|f| f.name == "app").ok_or("app not discovered")?;
        assert_eq!(
            app.providers,
            vec![
                Provider {
                    name: "money".to_string(),
                    path: PathBuf::from("deps/money"),
                },
                Provider {
                    name: "audit".to_string(),
                    path: PathBuf::from("deps/audit"),
                },
                Provider {
                    name: "ledger".to_string(),
                    path: PathBuf::from("deps/ledger"),
                },
            ]
        );
        let single = fixtures
            .iter()
            .find(|f| f.name == "single")
            .ok_or("single not discovered")?;
        assert!(single.providers.is_empty());
        Ok(())
    }

    /// A dependency cycle is refused naming the chain; a provider path outside the fixture is refused naming where it
    /// resolved to; a fixture manifest the compiler cannot read is refused with the compiler's words. A provider
    /// whose own manifest is unreadable is planned anyway: its bake reports the problem, attributed to the fixture.
    #[test]
    fn provider_plan_refuses_cycles_and_escapes_but_not_a_broken_provider() -> TestResult {
        let tmp = tempfile::tempdir()?;

        let cyclic = tmp.path().join("cyclic");
        write_project(&cyclic, "cyclic", &[("a", "deps/a")])?;
        fs::write(cyclic.join("src/main.incn"), "# behavior: b\n# expect-exit: 0\n")?;
        write_project(&cyclic.join("deps/a"), "a", &[("b", "../b")])?;
        write_project(&cyclic.join("deps/b"), "b", &[("a", "../a")])?;
        let error = parse_fixture(&cyclic).err().ok_or("a cycle must be refused")?;
        assert!(error.reason.contains("form a cycle"), "{error}");
        assert!(
            error.reason.contains("<fixture> -> deps/a -> deps/b -> deps/a"),
            "{error}"
        );

        let selfish = tmp.path().join("selfish");
        write_project(&selfish, "selfish", &[("me", ".")])?;
        fs::write(selfish.join("src/main.incn"), "# behavior: b\n# expect-exit: 0\n")?;
        let error = parse_fixture(&selfish)
            .err()
            .ok_or("a self-dependency must be refused")?;
        assert!(error.reason.contains("<fixture> -> <fixture>"), "{error}");

        let escaping = tmp.path().join("escaping");
        write_project(&escaping, "escaping", &[("outside", "../elsewhere")])?;
        fs::write(escaping.join("src/main.incn"), "# behavior: b\n# expect-exit: 0\n")?;
        let error = parse_fixture(&escaping)
            .err()
            .ok_or("an escaping path must be refused")?;
        assert!(error.reason.contains("outside the fixture directory"), "{error}");
        assert!(error.reason.contains("`outside` in `loaf.toml`"), "{error}");

        let unreadable = tmp.path().join("unreadable");
        fs::create_dir_all(unreadable.join("src"))?;
        fs::write(unreadable.join("loaf.toml"), "[project\nname = ")?;
        fs::write(unreadable.join("src/main.incn"), "# behavior: b\n# expect-exit: 0\n")?;
        let error = parse_fixture(&unreadable)
            .err()
            .ok_or("an unreadable manifest must be refused")?;
        assert!(
            error.reason.contains("cannot read the fixture's `loaf.toml`"),
            "{error}"
        );

        let broken_provider = tmp.path().join("broken_provider");
        write_project(&broken_provider, "broken_provider", &[("helper", "deps/helper")])?;
        fs::write(
            broken_provider.join("src/main.incn"),
            "# behavior: b\n# expect-exit: 0\n",
        )?;
        fs::create_dir_all(broken_provider.join("deps/helper"))?;
        fs::write(broken_provider.join("deps/helper/loaf.toml"), "[project\nname = ")?;
        let fixture = parse_fixture(&broken_provider)?;
        assert_eq!(
            fixture.providers,
            vec![Provider {
                name: "helper".to_string(),
                path: PathBuf::from("deps/helper"),
            }]
        );
        Ok(())
    }

    /// Lexical normalisation resolves `.` and `..` without the file system and keeps a `..` that climbs past the
    /// start, so an escaping path still fails to sit under the fixture.
    #[test]
    fn lexical_normalisation_resolves_dots() {
        assert_eq!(
            normalize_lexically(Path::new("/f/deps/ledger/../money/./src")),
            PathBuf::from("/f/deps/money/src")
        );
        assert_eq!(normalize_lexically(Path::new("a/../../b")), PathBuf::from("../b"));
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
