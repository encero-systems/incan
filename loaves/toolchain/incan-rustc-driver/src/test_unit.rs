//! Session test collection and the outcome policy passed to Oven beside a native test executable.

use std::path::{Path, PathBuf};

use crate::test_batch::{BatchPlace, SessionFixture};
use crate::test_conftest::Conftest;
use incan_driver::session::CompilationSession;
use incan_driver::testing::discovery::{CollectionEvalContext, discover_tests_and_fixtures_with_context};
use incan_driver::testing::types::TestMarker;
use incan_driver::unit::{UnitDescription, UnitKind};
use rustc_ast as ast;
use rustc_span::Span;

/// One collected test case's report name, native entry, parameters, outcome markers and selection markers, resolved
/// once by the session collector.
pub(crate) struct CollectedTest {
    pub(crate) name: String,
    // The zero-argument function the native harness registers: the test itself, or its generated case entry.
    pub(crate) entry: String,
    // In a file group, the function the group runs for this case; the runner selects cases by it.
    case: Option<String>,
    // A parametrized case's display values, which machine-readable reports carry.
    parameters: Vec<String>,
    skip: Option<String>,
    xfail: Option<String>,
    // `@slow`, which the runner leaves out unless asked, and the `@mark` names its marker expressions select.
    slow: bool,
    marks: Vec<String>,
    // Marks the test module does not register in `TEST_MARKERS`, which strict marker selection refuses.
    unregistered: Vec<String>,
    // `@timeout`, the test's own time limit in milliseconds, which the runner applies in place of its default.
    timeout_ms: Option<u64>,
    // `@resource` names the runner never holds by two running tests at once, and `@serial`, which runs alone.
    resources: Vec<String>,
    serial: bool,
    // In a batch unit, the test module the case belongs to.
    file: Option<PathBuf>,
    // The plain session fixtures the case's module shares with other modules below their conftest
    // (`crate::test_batch`), which the runner batches modules by.
    sessions: Vec<String>,
}

/// A test unit's collected cases, and each module checked with generated text: the entry module when any case needs
/// a generated entry, and in a batch every module and each conftest that hosts a shared fixture.
pub(crate) struct CollectedUnit {
    pub(crate) tests: Vec<CollectedTest>,
    pub(crate) sources: Vec<(PathBuf, String)>,
    /// In a batch, the directory the entry module's logical path is spelled from (`crate::test_batch`).
    pub(crate) frame: Option<PathBuf>,
}

/// Collect a test unit using the session's vocabulary and active features; unsupported harness contracts refuse by
/// name. An async test is collected like any other: the lowering registers a synchronous function that runs its
/// coroutine to completion on the canonical executor. A test that takes fixtures or `@parametrize` arguments runs as
/// one generated entry per case (`crate::test_cases`); the `conftest.incn` files from a test file's directory up to
/// `tests/` share their fixtures with it (`crate::test_conftest`). A unit with a test batch collects each of its
/// modules in run order (`crate::test_batch`).
pub(crate) fn collect(unit: &UnitDescription, session: &CompilationSession) -> Result<CollectedUnit, String> {
    let mut unit_collected = CollectedUnit {
        tests: Vec::new(),
        sources: Vec::new(),
        frame: None,
    };
    if unit.kind != UnitKind::Test {
        return Ok(unit_collected);
    }
    let context = CollectionEvalContext::new(unit.active_features.clone());
    let files = std::iter::once(&unit.source)
        .chain(unit.test_batch.iter())
        .map(|file| {
            file.canonicalize()
                .map_err(|error| format!("cannot resolve test module {}: {error}", file.display()))
        })
        .collect::<Result<Vec<PathBuf>, String>>()?;
    let batched = files.len() > 1;
    let chains = files
        .iter()
        .map(|file| Conftest::chain(file, &context, session))
        .collect::<Result<Vec<Vec<Conftest>>, String>>()?;
    if batched {
        let conftests = chains.iter().flatten().map(|conftest| conftest.path.as_path());
        unit_collected.frame = Some(crate::test_batch::frame(&files, conftests));
    }
    let mut hosted: Vec<SessionFixture> = Vec::new();
    for (position, (file, conftests)) in files.iter().zip(&chains).enumerate() {
        let place = unit_collected.frame.as_ref().map(|frame| BatchPlace {
            position,
            files: files.clone(),
            frame: frame.clone(),
        });
        let module = collect_module(file, conftests, place, &context, session)?;
        if batched && module.source.is_none() {
            return Err(format!("test batch module {} has no file group", file.display()));
        }
        unit_collected.tests.extend(module.tests);
        unit_collected
            .sources
            .extend(module.source.map(|text| (file.clone(), text)));
        for fixture in module.sessions {
            if !hosted.contains(&fixture) {
                hosted.push(fixture);
            }
        }
    }
    if batched {
        let conftests: Vec<&Conftest> = chains.iter().flatten().collect();
        unit_collected
            .sources
            .extend(crate::test_batch::conftest_sources(&hosted, &conftests, session)?);
    }
    Ok(unit_collected)
}

/// One test module's collected cases, its checked text and the shareable session fixtures it reaches.
struct CollectedModule {
    tests: Vec<CollectedTest>,
    source: Option<String>,
    sessions: Vec<SessionFixture>,
}

/// Collect one test module of a unit with its conftest chain; in a batch its cases name the module.
fn collect_module(
    file: &Path,
    conftests: &[Conftest],
    place: Option<BatchPlace>,
    context: &CollectionEvalContext,
    session: &CompilationSession,
) -> Result<CollectedModule, String> {
    let shared = Conftest::names(conftests);
    let discovery = discover_tests_and_fixtures_with_context(file, &shared, &[], &[], context, session)?;
    let batched = place.is_some();
    let planned = crate::test_cases::plan(file, session, &discovery, conftests, place)?;
    // Reports name paths below the project root: a stored unit's report is reused wherever the project lies.
    let project_root = &session.unit().project_root;
    let sessions: Vec<String> = planned
        .sessions
        .iter()
        .map(|fixture| fixture.key(project_root))
        .collect();
    let mut collected = Vec::new();
    for case in planned.cases {
        let test = discovery
            .tests
            .get(case.test)
            .ok_or("test case planning lost its test")?;
        let mut entry = CollectedTest {
            name: case.name,
            entry: case.entry,
            case: case.case,
            parameters: case.parameters,
            skip: None,
            xfail: None,
            slow: false,
            marks: Vec::new(),
            unregistered: Vec::new(),
            timeout_ms: test
                .timeout
                .map(|limit| u64::try_from(limit.as_millis()).unwrap_or(u64::MAX)),
            resources: Vec::new(),
            serial: false,
            file: batched.then(|| crate::test_batch::project_path(file, project_root).to_path_buf()),
            sessions: sessions.clone(),
        };
        for marker in test.markers.iter().chain(case.markers.iter()) {
            apply_marker(&mut entry, marker.clone(), &discovery.known_markers);
        }
        collected.push(entry);
    }
    Ok(CollectedModule {
        tests: collected,
        source: planned.source,
        sessions: planned.sessions,
    })
}

/// Record one marker on a collected case; a `param_case` marker applies to its case alone.
fn apply_marker(entry: &mut CollectedTest, marker: TestMarker, known_markers: &[String]) {
    match marker {
        TestMarker::Skip(reason) => entry.skip = Some(reason),
        TestMarker::XFail(reason) => entry.xfail = Some(reason),
        TestMarker::Slow => entry.slow = true,
        TestMarker::Mark(name) => {
            if !known_markers.contains(&name) && !entry.unregistered.contains(&name) {
                entry.unregistered.push(name.clone());
            }
            if !entry.marks.contains(&name) {
                entry.marks.push(name);
            }
        }
        TestMarker::Resource(name) => {
            if !entry.resources.contains(&name) {
                entry.resources.push(name);
            }
        }
        TestMarker::Serial => entry.serial = true,
        TestMarker::Timeout(limit) => {
            entry.timeout_ms = Some(u64::try_from(limit.as_millis()).unwrap_or(u64::MAX));
        }
        TestMarker::Test | TestMarker::Parametrize(..) => {}
    }
}

/// Pair checked native entries with collected policies without deriving discovery from emitted names.
pub(crate) fn report(tests: &[CollectedTest], native: &[String]) -> Result<serde_json::Value, String> {
    if tests.len() != native.len() {
        return Err("test lowering lost a collected entry".into());
    }
    Ok(serde_json::Value::Array(
        tests
            .iter()
            .zip(native)
            .map(|(test, native)| {
                serde_json::json!({
                    "name": test.name,
                    "native": native,
                    "case": test.case,
                    "parameters": test.parameters,
                    "skip": test.skip,
                    "xfail": test.xfail,
                    "slow": test.slow,
                    "marks": test.marks,
                    "unregistered": test.unregistered,
                    "timeout_ms": test.timeout_ms,
                    "resources": test.resources,
                    "serial": test.serial,
                    "file": test.file.as_ref().map(|file| file.display().to_string()),
                    "sessions": test.sessions,
                })
            })
            .collect(),
    ))
}

/// Register a checked function with rustc's native harness without parsing generated Rust source.
pub(crate) fn attribute(generator: &ast::attr::AttrIdGenerator, span: Span) -> ast::Attribute {
    ast::attr::mk_attr_from_item(
        generator,
        ast::AttrItem {
            unsafety: ast::Safety::Default,
            path: ast::Path::from_ident(crate::ast_build::ident("test", span)),
            args: ast::AttrItemKind::Unparsed(ast::AttrArgs::Empty),
            tokens: None,
        },
        None,
        ast::AttrStyle::Outer,
        span,
    )
}
