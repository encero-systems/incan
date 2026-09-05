//! Paired receipt-backed characterizations for builtin `abs` and `sum` overflow.
//!
//! The native-only module test observes the staged route independently. These assertions keep the two-route contract
//! separate. The panic guard turns an unexpected direct-executor panic into a test error so native evidence cannot be
//! mistaken for a paired comparison.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use incan::backend::replacement::ReplacementValue;
use incan::backend::selection::ShadowComparisonState;
use incan::backend::shadow::{
    RouteEvidence, RuntimeFailureClass, ShadowComparison, ShadowComparisonProfile, SourceObservable,
};
use incan::cli::commands::compare_source_observable;

#[path = "support/shadow_capability.rs"]
mod shadow_capability;

const ABS_MIN_SOURCE: &str = "def abs_min(value: int) -> int:\n    println(\"before abs\")\n    return abs(value)\n";
const SUM_OVERFLOW_SOURCE: &str =
    "def overflowing_sum() -> int:\n    println(\"before sum\")\n    return sum([9223372036854775807, 1])\n";

#[derive(Clone, Copy)]
struct OverflowCase {
    source: &'static str,
    function: &'static str,
    prefix: &'static [u8],
    abs_argument: bool,
}

const ABS_MIN: OverflowCase = OverflowCase {
    source: ABS_MIN_SOURCE,
    function: "abs_min",
    prefix: b"before abs\n",
    abs_argument: true,
};

const SUM_OVERFLOW: OverflowCase = OverflowCase {
    source: SUM_OVERFLOW_SOURCE,
    function: "overflowing_sum",
    prefix: b"before sum\n",
    abs_argument: false,
};

impl OverflowCase {
    /// Build the checked source and exact parameter values for this overflow case.
    fn profile(self) -> ShadowComparisonProfile {
        let arguments = if self.abs_argument {
            vec![ReplacementValue::Int(i64::MIN)]
        } else {
            Vec::new()
        };
        ShadowComparisonProfile::new(self.source, self.function, arguments)
    }
}

/// Run the CLI-owned paired comparator while turning any direct-route panic into a test error.
fn compare_without_direct_panic(
    profile: &ShadowComparisonProfile,
    capability: &incan::backend::shadow::legacy_oven::LegacyOvenCapability,
    workspace: &Path,
) -> Result<ShadowComparison, Box<dyn std::error::Error>> {
    match catch_unwind(AssertUnwindSafe(|| compare_source_observable(profile, capability, workspace))) {
        Ok(comparison) => Ok(comparison),
        Err(_) => Err(
            "direct Abs/Sum execution panicked before a paired comparison could run; run the native-only receipt test separately"
                .into(),
        ),
    }
}

/// Return both executed route records after confirming their independently bound receipts.
fn route_evidence(
    comparison: &ShadowComparison,
) -> Result<(&RouteEvidence, &RouteEvidence), Box<dyn std::error::Error>> {
    let legacy = comparison
        .legacy
        .as_ref()
        .ok_or_else(|| format!("expected executed legacy route, got {:?}", comparison.state))?;
    let replacement = comparison
        .replacement
        .as_ref()
        .ok_or_else(|| format!("expected executed replacement route, got {:?}", comparison.state))?;
    legacy.receipt()?.verify_identity()?;
    replacement.receipt()?.verify_identity()?;
    Ok((legacy, replacement))
}

/// Assert the common checked failure outcome, exact prefix, and intentionally distinct diagnostic streams.
fn assert_checked_divergence(
    comparison: &ShadowComparison,
    case: OverflowCase,
) -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        matches!(comparison.state, ShadowComparisonState::Diverged { .. }),
        "{}: {:?}",
        case.function,
        comparison.state
    );
    let (legacy, replacement) = route_evidence(comparison)?;
    let expected = SourceObservable::Failed {
        failure: RuntimeFailureClass::ArithmeticOverflow,
    };
    assert_eq!(legacy.observation.observable, expected);
    assert_eq!(replacement.observation.observable, expected);
    assert_eq!(legacy.observation.stdout, case.prefix);
    assert_eq!(replacement.observation.stdout, case.prefix);
    assert!(!legacy.observation.stderr.is_empty());
    assert!(replacement.observation.stderr.is_empty());
    assert!(comparison.replacement_execution.is_none());
    let process = comparison
        .legacy_process
        .as_ref()
        .ok_or("native process evidence must be retained")?;
    assert_eq!(process.stdout, case.prefix);
    assert_eq!(process.stderr, legacy.observation.stderr);
    assert!(process.result_report.is_none());
    Ok(())
}

/// Use the existing staged-capability convention: normal unstaged runs report a skip, while a required route fails.
fn staged_capability()
-> Result<Option<incan::backend::shadow::legacy_oven::LegacyOvenCapability>, Box<dyn std::error::Error>> {
    match shadow_capability::legacy_capability() {
        Ok(capability) => Ok(Some(capability)),
        Err(unavailable) if shadow_capability::legacy_route_is_required() => Err(format!(
            "{} is set but the native comparison route is unavailable: {}",
            shadow_capability::REQUIRE_LEGACY_ROUTE_ENV,
            unavailable.reason
        )
        .into()),
        Err(unavailable) => {
            eprintln!("skipping: {}", unavailable.reason);
            Ok(None)
        }
    }
}

/// The adopted Oven profile changes build mechanics, not the language-visible overflow outcome.
#[test]
fn abs_and_sum_comparisons_are_checked_under_the_adopted_oven_profile() -> Result<(), Box<dyn std::error::Error>> {
    let Some(capability) = staged_capability()? else {
        return Ok(());
    };
    for case in [ABS_MIN, SUM_OVERFLOW] {
        let workspace = tempfile::tempdir()?;
        let profile = case.profile();
        let comparison = compare_without_direct_panic(&profile, &capability, workspace.path())?;
        assert_checked_divergence(&comparison, case)?;
    }
    Ok(())
}
