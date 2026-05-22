//! Shared testing marker vocabulary.

use super::registry::{LangItemInfo, RFC, Since, Stability};
use super::stdlib;

/// Standard-library testing module segment.
pub const STDLIB_TESTING_MODULE: &str = "testing";

/// Stable identifier for a canonical `std.testing` assertion helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TestingAssertHelperId {
    Assert,
    AssertFalse,
    AssertEq,
    AssertNe,
    AssertIsSome,
    AssertIsNone,
    AssertIsOk,
    AssertIsErr,
    AssertRaises,
}

pub type TestingAssertHelperInfo = LangItemInfo<TestingAssertHelperId>;

/// Canonical `std.testing` assertion helpers with compiler-specialized emission.
pub const TESTING_ASSERT_HELPERS: &[TestingAssertHelperInfo] = &[
    assert_helper(TestingAssertHelperId::Assert, "assert"),
    assert_helper(TestingAssertHelperId::AssertFalse, "assert_false"),
    assert_helper(TestingAssertHelperId::AssertEq, "assert_eq"),
    assert_helper(TestingAssertHelperId::AssertNe, "assert_ne"),
    assert_helper(TestingAssertHelperId::AssertIsSome, "assert_is_some"),
    assert_helper(TestingAssertHelperId::AssertIsNone, "assert_is_none"),
    assert_helper(TestingAssertHelperId::AssertIsOk, "assert_is_ok"),
    assert_helper(TestingAssertHelperId::AssertIsErr, "assert_is_err"),
    assert_helper(TestingAssertHelperId::AssertRaises, "assert_raises"),
];

/// Resolve an assertion helper spelling to its stable id.
pub fn assert_helper_from_str(name: &str) -> Option<TestingAssertHelperId> {
    TESTING_ASSERT_HELPERS
        .iter()
        .find(|helper| helper.canonical == name)
        .map(|helper| helper.id)
}

/// Return the canonical spelling for an assertion helper id.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn assert_helper_as_str(id: TestingAssertHelperId) -> &'static str {
    TESTING_ASSERT_HELPERS
        .iter()
        .find(|helper| helper.id == id)
        .unwrap_or_else(|| panic!("testing assert helper info missing"))
        .canonical
}

/// Return the canonical fully qualified `std.testing` path for an assertion helper.
#[must_use]
pub fn assert_helper_path(id: TestingAssertHelperId) -> [&'static str; 3] {
    [stdlib::STDLIB_ROOT, STDLIB_TESTING_MODULE, assert_helper_as_str(id)]
}

/// Resolve a fully qualified `std.testing` path to an assertion helper id.
#[must_use]
pub fn assert_helper_id_from_std_path(path: &[String]) -> Option<TestingAssertHelperId> {
    let [root, module, name] = path else {
        return None;
    };
    if root == stdlib::STDLIB_ROOT && module == STDLIB_TESTING_MODULE {
        assert_helper_from_str(name)
    } else {
        None
    }
}

/// Return whether a fully qualified path names one specific `std.testing` assertion helper.
#[must_use]
pub fn is_assert_helper_std_path(path: &[String], id: TestingAssertHelperId) -> bool {
    assert_helper_id_from_std_path(path) == Some(id)
}

/// Return the default assertion failure text for helpers whose message does not depend on operands.
#[must_use]
pub fn assert_helper_default_failure_message(id: TestingAssertHelperId) -> Option<&'static str> {
    match id {
        TestingAssertHelperId::Assert | TestingAssertHelperId::AssertFalse => Some("AssertionError"),
        TestingAssertHelperId::AssertIsSome => Some("AssertionError: expected Some, got None"),
        TestingAssertHelperId::AssertIsNone => Some("AssertionError: expected None, got Some"),
        TestingAssertHelperId::AssertIsOk => Some("AssertionError: expected Ok, got Err"),
        TestingAssertHelperId::AssertIsErr => Some("AssertionError: expected Err, got Ok"),
        TestingAssertHelperId::AssertEq | TestingAssertHelperId::AssertNe | TestingAssertHelperId::AssertRaises => None,
    }
}

/// Return the operand relation text used by comparison assertion failures.
#[must_use]
pub fn assert_comparison_failure_kind(id: TestingAssertHelperId) -> Option<&'static str> {
    match id {
        TestingAssertHelperId::AssertEq => Some("left != right"),
        TestingAssertHelperId::AssertNe => Some("left == right"),
        _ => None,
    }
}

const fn assert_helper(id: TestingAssertHelperId, canonical: &'static str) -> TestingAssertHelperInfo {
    LangItemInfo {
        id,
        canonical,
        aliases: &[],
        description: "Canonical testing assertion helper.",
        introduced_in_rfc: RFC::_018,
        since: Since(0, 1),
        stability: Stability::Stable,
        examples: &[],
    }
}

/// Runtime marker name for `std.testing.test`.
pub const TESTING_MARKER_TEST: &str = "test";
/// Runtime marker name for `std.testing.fixture`.
pub const TESTING_MARKER_FIXTURE: &str = "fixture";
/// Runtime marker name for `std.testing.skip`.
pub const TESTING_MARKER_SKIP: &str = "skip";
/// Runtime marker name for `std.testing.skipif`.
pub const TESTING_MARKER_SKIPIF: &str = "skipif";
/// Runtime marker name for `std.testing.xfail`.
pub const TESTING_MARKER_XFAIL: &str = "xfail";
/// Runtime marker name for `std.testing.xfailif`.
pub const TESTING_MARKER_XFAILIF: &str = "xfailif";
/// Runtime marker name for `std.testing.slow`.
pub const TESTING_MARKER_SLOW: &str = "slow";
/// Runtime marker name for `std.testing.mark`.
pub const TESTING_MARKER_MARK: &str = "mark";
/// Runtime marker name for `std.testing.resource`.
pub const TESTING_MARKER_RESOURCE: &str = "resource";
/// Runtime marker name for `std.testing.serial`.
pub const TESTING_MARKER_SERIAL: &str = "serial";
/// Runtime marker name for `std.testing.timeout`.
pub const TESTING_MARKER_TIMEOUT: &str = "timeout";
/// Runtime marker name for `std.testing.parametrize`.
pub const TESTING_MARKER_PARAMETRIZE: &str = "parametrize";

/// Runner-only marker names that must have matching `@rust.extern` metadata in `stdlib/testing.incn`.
pub const RUNNER_ONLY_MARKER_NAMES: &[&str] = &[
    TESTING_MARKER_TEST,
    TESTING_MARKER_FIXTURE,
    TESTING_MARKER_SKIP,
    TESTING_MARKER_SKIPIF,
    TESTING_MARKER_XFAIL,
    TESTING_MARKER_XFAILIF,
    TESTING_MARKER_SLOW,
    TESTING_MARKER_MARK,
    TESTING_MARKER_RESOURCE,
    TESTING_MARKER_SERIAL,
    TESTING_MARKER_TIMEOUT,
    TESTING_MARKER_PARAMETRIZE,
];
