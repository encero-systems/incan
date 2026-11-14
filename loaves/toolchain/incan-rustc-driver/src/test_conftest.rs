//! Fixtures a test file receives from the `conftest.incn` beside it, as 0.5.1 shares them.
//!
//! A `conftest.incn` in the test file's own directory under `tests/` contributes its fixtures to the file's tests; a
//! fixture the test file declares itself takes precedence. The generated case entries (`crate::test_cases`) call a
//! shared fixture through an import of the `conftest` module, so the conftest is checked with each fixture the test
//! file uses made public: `pub ` is written before its `def`, on the same line, so every line keeps its number. A
//! conftest in a parent directory is not read, and a `yield` or async conftest fixture a test uses refuses by name.

use std::path::{Path, PathBuf};

use incan_driver::session::CompilationSession;
use incan_driver::testing::discovery::{CollectionEvalContext, discover_tests_and_fixtures_with_context};
use incan_driver::testing::types::{DiscoveryResult, FixtureInfo, TestInfo};
use incan_frontend::ast::{Declaration, Visibility};

/// The `conftest.incn` beside a test file and the fixtures it declares.
pub(crate) struct Conftest {
    pub(crate) path: PathBuf,
    pub(crate) discovery: DiscoveryResult,
}

impl Conftest {
    /// Find and collect the `conftest.incn` beside `test_file` when the file lies under a `tests` directory.
    pub(crate) fn beside(
        test_file: &Path,
        context: &CollectionEvalContext,
        session: &CompilationSession,
    ) -> Result<Option<Self>, String> {
        let under_tests = test_file
            .ancestors()
            .skip(1)
            .any(|directory| directory.file_name().and_then(|name| name.to_str()) == Some("tests"));
        let Some(path) = test_file.parent().map(|directory| directory.join("conftest.incn")) else {
            return Ok(None);
        };
        if !under_tests || !path.is_file() {
            return Ok(None);
        }
        let path = path
            .canonicalize()
            .map_err(|error| format!("cannot resolve {}: {error}", path.display()))?;
        let discovery = discover_tests_and_fixtures_with_context(&path, &[], &[], &[], context, session)?;
        Ok(Some(Self { path, discovery }))
    }

    /// Refuse, by name, a shared `yield` or async fixture that a test uses, directly, through another shared fixture or
    /// as an autouse fixture; one that no test uses stays out of the way.
    pub(crate) fn refuse_unsupported(&self, inherited: &[&FixtureInfo], tests: &[TestInfo]) -> Result<(), String> {
        for fixture in inherited
            .iter()
            .filter(|fixture| fixture.has_teardown || fixture.is_async)
        {
            let used = fixture.autouse
                || tests.iter().any(|test| test.required_fixtures.contains(&fixture.name))
                || self
                    .discovery
                    .fixtures
                    .iter()
                    .any(|other| other.dependencies.contains(&fixture.name));
            if used {
                return Err(format!(
                    "unsupported conftest fixture `{}` on the native harness: a shared fixture cannot `yield` or be async",
                    fixture.name
                ));
            }
        }
        Ok(())
    }

    /// The fixture names the conftest offers a test file.
    pub(crate) fn names(&self) -> Vec<String> {
        self.discovery
            .fixtures
            .iter()
            .map(|fixture| fixture.name.clone())
            .collect()
    }

    /// The conftest's fixtures a test file does not declare itself, which its entries call through the import.
    pub(crate) fn inherited<'a>(&'a self, own: &[FixtureInfo]) -> Vec<&'a FixtureInfo> {
        self.discovery
            .fixtures
            .iter()
            .filter(|fixture| !own.iter().any(|declared| declared.name == fixture.name))
            .collect()
    }

    /// The conftest's text with each of `names` that is private made public and each `yield` fixture blanked, as a
    /// test file's own are (`crate::test_cases`), keeping every line break.
    pub(crate) fn exported_source(&self, names: &[&str], session: &CompilationSession) -> Result<String, String> {
        let source = std::fs::read_to_string(&self.path)
            .map_err(|error| format!("cannot read {}: {error}", self.path.display()))?;
        let program = session
            .parse_source(&self.path, &source, false)
            .map_err(|errors| format!("cannot parse {}: {errors:?}", self.path.display()))?;
        let mut insertions = Vec::new();
        let mut blanked = Vec::new();
        for declaration in &program.declarations {
            let Declaration::Function(function) = &declaration.node else {
                continue;
            };
            if self
                .discovery
                .fixtures
                .iter()
                .any(|fixture| fixture.has_teardown && fixture.name == function.name)
            {
                let mut start = function
                    .decorators
                    .iter()
                    .map(|decorator| decorator.span.start)
                    .fold(declaration.span.start, usize::min);
                if start > 0 && source.as_bytes().get(start - 1) == Some(&b'@') {
                    start -= 1;
                }
                blanked.push((start, declaration.span.end));
                continue;
            }
            if matches!(function.visibility, Visibility::Public) || !names.contains(&function.name.as_str()) {
                continue;
            }
            // The `def` follows the function's decorators; the declaration may start at the first of them.
            let after_decorators = function
                .decorators
                .iter()
                .map(|decorator| decorator.span.end)
                .fold(declaration.span.start, usize::max);
            let keyword = source
                .get(after_decorators..)
                .and_then(|rest| rest.find("def "))
                .map(|offset| after_decorators + offset)
                .ok_or_else(|| format!("conftest fixture `{}` has no `def`", function.name))?;
            insertions.push(keyword);
        }
        let mut exported = String::with_capacity(source.len() + 4 * insertions.len());
        for (offset, character) in source.char_indices() {
            if insertions.contains(&offset) {
                exported.push_str("pub ");
            }
            let hidden = blanked.iter().any(|(start, end)| offset >= *start && offset < *end);
            exported.push(if hidden && character != '\n' { ' ' } else { character });
        }
        Ok(exported)
    }
}
