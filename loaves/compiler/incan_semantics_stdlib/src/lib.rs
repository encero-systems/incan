//! Stdlib semantics pack for parser/typechecker/lowering dispatch.
//!
//! This crate provides [`StdlibSemanticsPack`], the default [`SurfaceSemanticsPack`] implementation that maps Incan's
//! stdlib soft keywords and decorators to their pipeline behaviors.
//!
//! ## Feature gating
//!
//! Each stdlib capability is gated behind a Cargo feature so the main `incan` crate can include only the semantics it
//! needs at compile time:
//!
//! | Feature            | Enables                                                  |
//! | ------------------ | -------------------------------------------------------- |
//! | `std_testing`      | `assert` keyword → `std.testing.assert_*` call targets   |
//! | `std_async`        | `async` modifier, `await` prefix expression              |
//! | `std_decorators`   | Decorator-path classification (known + stdlib functions) |
//!
//! When a feature is disabled, the corresponding [`SurfaceSemanticsPack`] methods return `None` and the compiler
//! behaves as if that stdlib namespace does not exist.
//!
//! ## Adding a new handler
//!
//! 1. Add a `#[cfg(feature = "your_feature")]` block inside the appropriate trait method.
//! 2. Add the feature name to this crate's `Cargo.toml` `[features]` and wire it through the main crate's feature
//!    flags.
//! 3. See [`incan_semantics_core::SurfaceSemanticsPack`] for the trait contract.

use incan_core::lang::decorators;
use incan_core::lang::keywords::KeywordId;
use incan_core::lang::stdlib;
use incan_semantics_core::{
    AssertShape, DecoratorFeature, RuntimeRequirement, SurfaceCallTarget, SurfaceExprLoweringAction,
    SurfaceExprPayloadKind, SurfaceExprTypeCheck, SurfaceFeatureKey, SurfaceModifierKind, SurfaceModifierTypeCheck,
    SurfaceSemanticsPack, SurfaceStmtLoweringAction, SurfaceStmtPayloadKind, SurfaceStmtTypeCheck,
    decorator_feature_from_id,
};

/// Stdlib semantics pack implementation.
#[derive(Debug, Default)]
pub struct StdlibSemanticsPack;

impl StdlibSemanticsPack {
    pub fn new() -> Self {
        Self
    }
}

impl SurfaceSemanticsPack for StdlibSemanticsPack {
    fn statement_payload_for_soft_keyword(&self, keyword: KeywordId) -> Option<SurfaceStmtPayloadKind> {
        #[cfg(feature = "std_testing")]
        {
            if keyword == KeywordId::Assert {
                return Some(SurfaceStmtPayloadKind::KeywordArgs);
            }
        }
        None
    }

    fn expression_payload_for_soft_keyword(&self, keyword: KeywordId) -> Option<SurfaceExprPayloadKind> {
        #[cfg(feature = "std_async")]
        {
            if keyword == KeywordId::Await {
                return Some(SurfaceExprPayloadKind::PrefixUnary);
            }
        }
        None
    }

    fn modifier_payload_for_soft_keyword(&self, keyword: KeywordId) -> Option<SurfaceModifierKind> {
        #[cfg(feature = "std_async")]
        {
            if keyword == KeywordId::Async {
                return Some(SurfaceModifierKind::PrefixMarker);
            }
        }
        None
    }

    fn decorator_feature_for_path(&self, resolved_path: &[String]) -> Option<DecoratorFeature> {
        #[cfg(feature = "std_decorators")]
        {
            if let Some(id) = decorators::from_segments(resolved_path) {
                return Some(decorator_feature_from_id(id));
            }
        }
        #[cfg(feature = "std_decorators")]
        {
            // Provider catalogs validate whether the module and function exist. The semantics pack only classifies
            // the shape here so a newly published `std.*` provider decorator does not need a compiler registry entry.
            if resolved_path.len() >= 3 && resolved_path[0] == stdlib::STDLIB_ROOT {
                return Some(DecoratorFeature::StdlibDecoratorFunction);
            }
        }
        None
    }

    fn assert_call_target(&self, shape: AssertShape) -> Option<SurfaceCallTarget> {
        #[cfg(feature = "std_testing")]
        {
            let local_name = match shape {
                AssertShape::Condition => "assert",
                AssertShape::Eq => "assert_eq",
                AssertShape::Ne => "assert_ne",
                AssertShape::Not => "assert_false",
            };
            return Some(SurfaceCallTarget {
                local_name,
                canonical_path: vec![
                    stdlib::STDLIB_ROOT.to_string(),
                    "testing".to_string(),
                    local_name.to_string(),
                ],
            });
        }
        // When `std_testing` feature is enabled the early return above makes this unreachable,
        // but without the feature this fallthrough is needed.
        #[allow(unreachable_code)]
        None
    }

    // ---- Typechecker dispatch ----

    fn typecheck_surface_stmt_action(&self, key: &SurfaceFeatureKey) -> Option<SurfaceStmtTypeCheck> {
        #[cfg(feature = "std_testing")]
        if matches!(key, SurfaceFeatureKey::SoftKeyword(KeywordId::Assert)) {
            return Some(SurfaceStmtTypeCheck::AssertCheck);
        }
        let _ = key; // suppress unused warning when no features are active
        None
    }

    /// Route stdlib-owned surface expressions to their typechecker actions.
    fn typecheck_surface_expr_action(&self, key: &SurfaceFeatureKey) -> Option<SurfaceExprTypeCheck> {
        #[cfg(feature = "std_async")]
        if matches!(key, SurfaceFeatureKey::SoftKeyword(KeywordId::Await)) {
            return Some(SurfaceExprTypeCheck::AwaitCheck);
        }
        #[cfg(feature = "std_async")]
        if is_std_async_race_for_surface(key) {
            return Some(SurfaceExprTypeCheck::RaceForCheck);
        }
        let _ = key;
        None
    }

    /// Route `async` declaration modifiers to async callable validation when `std_async` is enabled.
    fn typecheck_surface_modifier_action(&self, key: &SurfaceFeatureKey) -> Option<SurfaceModifierTypeCheck> {
        #[cfg(feature = "std_async")]
        if matches!(key, SurfaceFeatureKey::SoftKeyword(KeywordId::Async)) {
            return Some(SurfaceModifierTypeCheck::AsyncCallable);
        }
        let _ = key;
        None
    }

    // ---- Lowering dispatch ----

    fn lower_surface_stmt_action(&self, key: &SurfaceFeatureKey) -> Option<SurfaceStmtLoweringAction> {
        #[cfg(feature = "std_testing")]
        if matches!(key, SurfaceFeatureKey::SoftKeyword(KeywordId::Assert)) {
            return Some(SurfaceStmtLoweringAction::AssertCall);
        }
        let _ = key;
        None
    }

    /// Route stdlib-owned surface expressions to their lowering actions.
    fn lower_surface_expr_action(&self, key: &SurfaceFeatureKey) -> Option<SurfaceExprLoweringAction> {
        #[cfg(feature = "std_async")]
        if matches!(key, SurfaceFeatureKey::SoftKeyword(KeywordId::Await)) {
            return Some(SurfaceExprLoweringAction::Await);
        }
        #[cfg(feature = "std_async")]
        if is_std_async_race_for_surface(key) {
            return Some(SurfaceExprLoweringAction::RaceFor);
        }
        let _ = key;
        None
    }

    // ---- Runtime requirement scanning ----

    fn modifier_runtime_requirement(&self, key: &SurfaceFeatureKey) -> RuntimeRequirement {
        #[cfg(feature = "std_async")]
        if matches!(
            key,
            SurfaceFeatureKey::SoftKeyword(KeywordId::Async) | SurfaceFeatureKey::SoftKeyword(KeywordId::Await)
        ) {
            return RuntimeRequirement::AsyncRuntime;
        }
        let _ = key;
        RuntimeRequirement::None
    }

    fn import_runtime_requirement(&self, module: &str) -> RuntimeRequirement {
        #[cfg(feature = "std_async")]
        if module == "async" {
            return RuntimeRequirement::AsyncRuntime;
        }
        let _ = module;
        RuntimeRequirement::None
    }
}

/// Return whether a surface key names the `std.async` race expression.
fn is_std_async_race_for_surface(key: &SurfaceFeatureKey) -> bool {
    matches!(
        key,
        SurfaceFeatureKey::ScopedDslSurface {
            dependency_key,
            descriptor_key,
        } if dependency_key == "std.async" && descriptor_key == "race_for"
    )
}
