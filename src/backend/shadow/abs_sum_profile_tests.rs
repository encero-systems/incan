//! Receipt-authorized native observations for the bounded builtin `abs`/`sum` overflow repair.
//!
//! This module intentionally observes only the legacy route. The paired integration test owns the two-route contract,
//! while this module independently qualifies the receipt-selected native overflow profile.

use crate::backend::replacement::ReplacementValue;
use crate::backend::shadow::legacy_oven::{self, LegacyOvenCapability};
use crate::provider::FeatureSelection;

use super::{LegacyRouteResult, PreparedShadowProfile, RuntimeFailureClass, ShadowComparisonProfile, SourceObservable};

const ABS_MIN_SOURCE: &str = "def abs_min(value: int) -> int:\n    println(\"before abs\")\n    return abs(value)\n";
const SUM_OVERFLOW_SOURCE: &str =
    "def overflowing_sum() -> int:\n    println(\"before sum\")\n    return sum([9223372036854775807, 1])\n";
const REQUIRE_LEGACY_ROUTE_ENV: &str = "INCAN_SHADOW_REQUIRE_LEGACY_ROUTE";

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
    /// Build the checked source and exact parameter values for this native overflow observation.
    fn profile(self) -> ShadowComparisonProfile {
        let arguments = if self.abs_argument {
            vec![ReplacementValue::Int(i64::MIN)]
        } else {
            Vec::new()
        };
        ShadowComparisonProfile::new(self.source, self.function, arguments)
    }
}

/// Resolve the normal staged Oven capability, reporting unstaged local runs without hiding required-route failures.
fn staged_capability() -> Result<Option<LegacyOvenCapability>, Box<dyn std::error::Error>> {
    match LegacyOvenCapability::from_environment() {
        Ok(capability) => Ok(Some(capability)),
        Err(unavailable)
            if std::env::var_os(REQUIRE_LEGACY_ROUTE_ENV).is_some_and(|value| !value.is_empty() && value != "0") =>
        {
            Err(format!(
                "{REQUIRE_LEGACY_ROUTE_ENV} is set but the native Abs/Sum route is unavailable: {}",
                unavailable.reason
            )
            .into())
        }
        Err(unavailable) => {
            eprintln!("skipping: {}", unavailable.reason);
            Ok(None)
        }
    }
}

/// Observe the adopted native route without invoking the direct executor.
fn observe_native(
    case: OverflowCase,
    capability: &LegacyOvenCapability,
) -> Result<LegacyRouteResult, Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let profile = case.profile();
    let prepared = PreparedShadowProfile::new(&profile)?;
    let source_path = workspace.path().join("native-abs-sum-profile.incn");
    std::fs::write(&source_path, profile.source())?;
    let materialization = crate::cli::commands::shadow_support::prepare_shadow_legacy_materialization(
        &source_path,
        &FeatureSelection::default(),
        None,
    )?;
    let route = legacy_oven::observe_legacy_route(&profile, &prepared, &materialization, capability, workspace.path())?;
    assert!(route.authority.oven_receipt_identity.starts_with("sha256:"));
    assert!(route.authority.oven_build_unit_identity.starts_with("sha256:"));
    assert!(route.authority.direct_rustc_plan_identity.starts_with("sha256:"));
    assert!(route.authority.output_digest.starts_with("sha256:"));
    assert!(!route.authority.cargo_process_started);
    Ok(route)
}

/// Check the profile-independent native failure without reducing its diagnostic to a direct-route approximation.
fn assert_checked_overflow(route: &LegacyRouteResult, case: OverflowCase) -> Result<(), Box<dyn std::error::Error>> {
    assert_ne!(route.process.exit_code, Some(0));
    assert_eq!(route.process.stdout, case.prefix);
    assert!(!route.process.stderr.is_empty());
    assert!(route.process.result_report.is_none());

    let observation = route.observation.as_ref().ok_or_else(|| {
        format!(
            "native {} did not yield a classifiable overflow; exit={:?}, stderr={:?}, unavailable={:?}",
            case.function, route.process.exit_code, route.process.stderr, route.unavailable_reason
        )
    })?;
    assert_eq!(
        observation.observable,
        SourceObservable::Failed {
            failure: RuntimeFailureClass::ArithmeticOverflow,
        }
    );
    assert_eq!(observation.stdout, case.prefix);
    assert_eq!(observation.stderr, route.process.stderr);
    Ok(())
}

/// Any verified Oven profile executes the same checked builtin contract.
#[test]
fn native_abs_and_sum_are_checked_under_the_adopted_oven_profile() -> Result<(), Box<dyn std::error::Error>> {
    let Some(capability) = staged_capability()? else {
        return Ok(());
    };
    for case in [ABS_MIN, SUM_OVERFLOW] {
        let route = observe_native(case, &capability)?;
        assert_checked_overflow(&route, case)?;
    }
    Ok(())
}
