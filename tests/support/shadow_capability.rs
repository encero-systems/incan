//! Resolve the Oven capability the #1146 legacy comparison route needs, for tests that want to run it.
//!
//! The legacy route is Oven-owned: it needs a bounded store holding a published direct-`rustc` plan, a verified
//! receipt from the project that plan was baked for, and an explicit compiler. Staging those is an operator step
//! (`incan oven bake`), so this module only *reads* the staging contract that
//! `incan::backend::shadow::legacy_oven` already defines — it never substitutes a compiler invocation of its own.
//!
//! When nothing is staged, callers get the same [`ShadowUnavailable`] the compiler would record, so an unstaged
//! environment produces an explicit non-green reason rather than a silent skip.

use std::path::Path;

use incan::backend::shadow::ShadowLegacyMaterialization;
use incan::backend::shadow::ShadowUnavailable;
use incan::backend::shadow::legacy_oven::LegacyOvenCapability;
use incan::cli::commands::prepare_shadow_legacy_materialization;
use incan::provider::FeatureSelection;

/// Resolve the staged legacy capability, or the concrete reason there is none.
#[allow(dead_code)]
pub(crate) fn legacy_capability() -> Result<LegacyOvenCapability, ShadowUnavailable> {
    LegacyOvenCapability::from_environment()
}

/// Select the staged receipt whose exact immutable inputs match one source-session materialization.
#[allow(dead_code)]
pub(crate) fn legacy_capability_for(
    materialization: &ShadowLegacyMaterialization,
) -> Result<LegacyOvenCapability, ShadowUnavailable> {
    LegacyOvenCapability::from_environment_for_materialization(materialization)
}

/// Resolve one final Oven-compatible provider plan from the exact source profile the comparison observes.
///
/// The CLI owns discovery because the backend must not recreate a compilation session or infer provider ownership
/// from an Oven receipt. A missing source or inventory remains an explicit unavailable result.
#[allow(dead_code)]
pub(crate) fn legacy_materialization(entry_path: &Path) -> Result<ShadowLegacyMaterialization, ShadowUnavailable> {
    prepare_shadow_legacy_materialization(entry_path, &FeatureSelection::default(), None)
}

/// Environment variable that turns a missing legacy capability into a failure instead of a reported skip.
///
/// A comparison that cannot run is an honest non-green result, so an unstaged developer machine reports the
/// reason and moves on. CI, which is expected to stage the Oven, sets this so an accidentally unstaged run fails
/// loudly rather than passing without proving anything.
pub(crate) const REQUIRE_LEGACY_ROUTE_ENV: &str = "INCAN_SHADOW_REQUIRE_LEGACY_ROUTE";

/// Whether this environment demands a staged legacy route.
#[allow(dead_code)]
pub(crate) fn legacy_route_is_required() -> bool {
    std::env::var_os(REQUIRE_LEGACY_ROUTE_ENV).is_some_and(|value| !value.is_empty() && value != "0")
}

/// Report why a comparison could not run, failing when this environment demands one.
///
/// Returns `Ok(Some(reason))` when the caller should report the optional skip and stop, and `Ok(None)` when the
/// legacy route is staged and the caller must proceed. A required but unstaged route returns an error so test
/// harnesses preserve their ordinary `Result` failure flow.
#[allow(dead_code)]
pub(crate) fn unstaged_legacy_route_reason() -> Result<Option<String>, ShadowUnavailable> {
    match legacy_capability() {
        Ok(_) => Ok(None),
        Err(unavailable) if legacy_route_is_required() => Err(ShadowUnavailable::new(format!(
            "{REQUIRE_LEGACY_ROUTE_ENV} is set but the legacy comparison route is not staged: {}",
            unavailable.reason
        ))),
        Err(unavailable) => Ok(Some(unavailable.reason)),
    }
}
