//! Session test collection and the outcome policy passed to Oven beside a native test executable.

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
}

/// A test unit's collected cases, and the source its entries are checked in when any case needs a generated entry.
pub(crate) struct CollectedUnit {
    pub(crate) tests: Vec<CollectedTest>,
    pub(crate) source: Option<String>,
}

/// Collect a test unit using the session's vocabulary and active features; unsupported harness contracts refuse by
/// name. An async test is collected like any other: the lowering registers a synchronous function that runs its
/// coroutine to completion on the canonical executor. A test that takes fixtures or `@parametrize` arguments runs as
/// one generated entry per case (`crate::test_cases`); the `conftest.incn` files from a test file's directory up to
/// `tests/` share their fixtures with it (`crate::test_conftest`).
pub(crate) fn collect(unit: &UnitDescription, session: &CompilationSession) -> Result<CollectedUnit, String> {
    if unit.kind != UnitKind::Test {
        return Ok(CollectedUnit {
            tests: Vec::new(),
            source: None,
        });
    }
    let context = CollectionEvalContext::new(unit.active_features.clone());
    let conftests = Conftest::chain(&unit.source, &context, session)?;
    let shared = Conftest::names(&conftests);
    let discovery = discover_tests_and_fixtures_with_context(&unit.source, &shared, &[], &[], &context, session)?;
    let planned = crate::test_cases::plan(&unit.source, session, &discovery, &conftests)?;
    let mut collected = Vec::new();
    for case in planned.cases {
        let test = discovery
            .tests
            .get(case.test)
            .ok_or("test case planning lost its test")?;
        let mut entry = CollectedTest {
            name: case.name,
            entry: case.entry,
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
        };
        for marker in test.markers.iter().chain(case.markers.iter()) {
            apply_marker(&mut entry, marker.clone(), &discovery.known_markers);
        }
        collected.push(entry);
    }
    Ok(CollectedUnit {
        tests: collected,
        source: planned.source,
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
                    "parameters": test.parameters,
                    "skip": test.skip,
                    "xfail": test.xfail,
                    "slow": test.slow,
                    "marks": test.marks,
                    "unregistered": test.unregistered,
                    "timeout_ms": test.timeout_ms,
                    "resources": test.resources,
                    "serial": test.serial,
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
