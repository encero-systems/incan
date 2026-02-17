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
    SurfaceExprPayloadKind, SurfaceExprTypeCheck, SurfaceFeatureKey, SurfaceModifierKind, SurfaceSemanticsPack,
    SurfaceStmtLoweringAction, SurfaceStmtPayloadKind, SurfaceStmtTypeCheck, decorator_feature_from_id,
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
            if resolved_path.len() >= 3
                && resolved_path[0] == stdlib::STDLIB_ROOT
                && stdlib::is_known_stdlib_module(&resolved_path[..resolved_path.len() - 1])
            {
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

    fn typecheck_surface_expr_action(&self, key: &SurfaceFeatureKey) -> Option<SurfaceExprTypeCheck> {
        #[cfg(feature = "std_async")]
        if matches!(key, SurfaceFeatureKey::SoftKeyword(KeywordId::Await)) {
            return Some(SurfaceExprTypeCheck::AwaitCheck);
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

    fn lower_surface_expr_action(&self, key: &SurfaceFeatureKey) -> Option<SurfaceExprLoweringAction> {
        #[cfg(feature = "std_async")]
        if matches!(key, SurfaceFeatureKey::SoftKeyword(KeywordId::Await)) {
            return Some(SurfaceExprLoweringAction::Await);
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
