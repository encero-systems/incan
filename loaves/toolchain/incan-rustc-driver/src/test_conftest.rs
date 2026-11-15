//! Fixtures a test file receives from `conftest.incn` files, as 0.5.1 shares them.
//!
//! Every `conftest.incn` from the test file's directory up to the `tests` directory above it contributes its fixtures;
//! a nearer conftest's fixture takes precedence over a farther one's, and a fixture the test file declares itself over
//! both. As 0.5.1 compiled a test file together with its conftests, the checked test file carries each conftest's text
//! after its own, with every shadowed fixture blanked and every import the file or a nearer conftest already makes
//! blanked too, so plain, `yield` and async shared fixtures are ordinary declarations beside the tests.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use incan_driver::session::CompilationSession;
use incan_driver::testing::discovery::{CollectionEvalContext, discover_tests_and_fixtures_with_context};
use incan_driver::testing::types::{DiscoveryResult, FixtureInfo};
use incan_frontend::ast::Declaration;
use incan_frontend::decorator_resolution::collect_declaration_import_aliases;

/// One `conftest.incn` a test file receives fixtures from, and the fixtures it declares.
pub(crate) struct Conftest {
    pub(crate) path: PathBuf,
    pub(crate) discovery: DiscoveryResult,
}

impl Conftest {
    /// The conftests of `test_file`, nearest first: its own directory's, then each parent's up to the nearest `tests`
    /// directory above it. Each is collected seeing its parents' fixtures, which it may depend on.
    pub(crate) fn chain(
        test_file: &Path,
        context: &CollectionEvalContext,
        session: &CompilationSession,
    ) -> Result<Vec<Self>, String> {
        let Some(root) = test_file
            .ancestors()
            .skip(1)
            .find(|directory| directory.file_name().and_then(|name| name.to_str()) == Some("tests"))
        else {
            return Ok(Vec::new());
        };
        let directories: Vec<&Path> = test_file
            .ancestors()
            .skip(1)
            .take_while(|directory| directory.starts_with(root))
            .collect();
        let mut chain = Vec::new();
        let mut visible: Vec<String> = Vec::new();
        for directory in directories.iter().rev() {
            let path = directory.join("conftest.incn");
            if !path.is_file() {
                continue;
            }
            let path = path
                .canonicalize()
                .map_err(|error| format!("cannot resolve {}: {error}", path.display()))?;
            let discovery = discover_tests_and_fixtures_with_context(&path, &visible, &[], &[], context, session)?;
            visible.extend(discovery.fixtures.iter().map(|fixture| fixture.name.clone()));
            chain.push(Self { path, discovery });
        }
        chain.reverse();
        Ok(chain)
    }

    /// Every fixture name a chain offers a test file.
    pub(crate) fn names(chain: &[Self]) -> Vec<String> {
        chain
            .iter()
            .flat_map(|conftest| conftest.discovery.fixtures.iter().map(|fixture| fixture.name.clone()))
            .collect()
    }
}

/// A chain's text to append to a test file, and the shared fixtures it keeps, nearest conftest first.
///
/// 1. A fixture the test file or a nearer conftest declares shadows a farther one's: the farther one is blanked.
/// 2. An import whose every name is already imported, by the test file or a nearer conftest, and a conftest's module
///    docstring are blanked.
/// 3. Every other line is kept, so a shared fixture still sees its own conftest's imports and helpers.
pub(crate) fn appended<'a>(
    chain: &'a [Conftest],
    own_fixtures: &[FixtureInfo],
    own_imports: HashSet<String>,
    session: &CompilationSession,
) -> Result<(String, Vec<&'a FixtureInfo>), String> {
    let mut declared: HashSet<String> = own_fixtures.iter().map(|fixture| fixture.name.clone()).collect();
    let mut imported = own_imports;
    let mut text = String::new();
    let mut kept = Vec::new();
    for conftest in chain {
        let source = std::fs::read_to_string(&conftest.path)
            .map_err(|error| format!("cannot read {}: {error}", conftest.path.display()))?;
        let program = session
            .parse_source(&conftest.path, &source, false)
            .map_err(|errors| format!("cannot parse {}: {errors:?}", conftest.path.display()))?;
        let mut regions = Vec::new();
        for declaration in &program.declarations {
            match &declaration.node {
                // 1. A shadowed fixture.
                Declaration::Function(function)
                    if declared.contains(&function.name)
                        && conftest
                            .discovery
                            .fixtures
                            .iter()
                            .any(|fixture| fixture.name == function.name) =>
                {
                    regions.push(crate::test_entries::declaration_region(&source, declaration, function));
                }
                // A conftest's module docstring belongs at the top of its own file only.
                Declaration::Docstring(_) => regions.push((declaration.span.start, declaration.span.end)),
                // 2. An import the checked file already makes.
                Declaration::Import(_) => {
                    let names = collect_declaration_import_aliases(std::slice::from_ref(declaration));
                    if !names.is_empty() && names.keys().all(|name| imported.contains(name)) {
                        regions.push((declaration.span.start, declaration.span.end));
                    }
                    imported.extend(names.into_keys());
                }
                _ => {}
            }
        }
        for fixture in &conftest.discovery.fixtures {
            if declared.insert(fixture.name.clone()) {
                kept.push(fixture);
            }
        }
        // 3. The rest of the conftest, after the file and any nearer conftest.
        text.push_str("\n\n");
        text.push_str(&crate::test_entries::blank(&source, &regions));
    }
    Ok((text, kept))
}
