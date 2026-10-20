//! Session test collection and the outcome policy passed to Oven beside a native test executable.

use incan_driver::session::CompilationSession;
use incan_driver::testing::discovery::{CollectionEvalContext, discover_tests_and_fixtures_with_context};
use incan_driver::testing::types::TestMarker;
use incan_driver::unit::{UnitDescription, UnitKind};
use rustc_ast as ast;
use rustc_span::Span;

/// One collected test's source name and outcome markers, resolved once by the session collector.
pub(crate) struct CollectedTest {
    pub(crate) name: String,
    skip: Option<String>,
    xfail: Option<String>,
}

/// Collect a test unit using the session's vocabulary and active features; unsupported harness contracts refuse by
/// name.
pub(crate) fn collect(unit: &UnitDescription, session: &CompilationSession) -> Result<Vec<CollectedTest>, String> {
    if unit.kind != UnitKind::Test {
        return Ok(Vec::new());
    }
    let discovery = discover_tests_and_fixtures_with_context(
        &unit.source,
        &[],
        &[],
        &[],
        &CollectionEvalContext::new(unit.active_features.clone()),
        session,
    )?;
    if !discovery.fixtures.is_empty() {
        return Err("unsupported test unit fixtures on the native harness".into());
    }
    let mut collected = Vec::new();
    for test in discovery.tests {
        if test.is_async || !test.parameter_names.is_empty() || test.timeout.is_some() {
            return Err(format!(
                "unsupported test contract for `{}`: async, parameters or timeout",
                test.function_name
            ));
        }
        let mut entry = CollectedTest {
            name: test.function_name,
            skip: None,
            xfail: None,
        };
        for marker in test.markers {
            match marker {
                TestMarker::Skip(reason) => entry.skip = Some(reason),
                TestMarker::XFail(reason) => entry.xfail = Some(reason),
                TestMarker::Test | TestMarker::Slow | TestMarker::Mark(_) | TestMarker::Serial => {}
                _ => return Err(format!("unsupported test marker for `{}`", entry.name)),
            }
        }
        collected.push(entry);
    }
    Ok(collected)
}

/// Pair checked native entries with collected policies without deriving discovery from emitted names.
pub(crate) fn report(tests: &[CollectedTest], native: &[String]) -> Result<serde_json::Value, String> {
    if tests.len() != native.len() {
        return Err("test lowering lost a collected entry".into());
    }
    Ok(serde_json::Value::Array(tests.iter().zip(native).map(|(test, native)| {
        serde_json::json!({"name": test.name, "native": native, "skip": test.skip, "xfail": test.xfail})
    }).collect()))
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
