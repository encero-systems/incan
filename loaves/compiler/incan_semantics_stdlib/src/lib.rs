//! Stdlib semantics pack for parser/typechecker/lowering dispatch.
//!
//! This crate provides [`StdlibSemanticsPack`], the default [`SurfaceSemanticsPack`] implementation that maps Incan's
//! stdlib soft keywords and decorators to their pipeline behaviors.
//!
//! All compiler consumers share the complete pack. Source imports and provider catalogs determine which surface
//! constructs are available to a program; compiler build features do not change that contract.

use incan_lang::lang::decorators;
use incan_lang::lang::keywords::KeywordId;
use incan_lang::lang::stdlib;
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
    /// Construct the standard surface-semantics pack shared by compiler consumers.
    pub fn new() -> Self {
        Self
    }
}

impl SurfaceSemanticsPack for StdlibSemanticsPack {
    /// Classify the argument payload of the core assertion statement.
    fn statement_payload_for_soft_keyword(&self, keyword: KeywordId) -> Option<SurfaceStmtPayloadKind> {
        if keyword == KeywordId::Assert {
            return Some(SurfaceStmtPayloadKind::KeywordArgs);
        }
        None
    }

    /// Classify the operand payload of the activated await expression.
    fn expression_payload_for_soft_keyword(&self, keyword: KeywordId) -> Option<SurfaceExprPayloadKind> {
        if keyword == KeywordId::Await {
            return Some(SurfaceExprPayloadKind::PrefixUnary);
        }
        None
    }

    /// Classify the marker payload of an activated async declaration modifier.
    fn modifier_payload_for_soft_keyword(&self, keyword: KeywordId) -> Option<SurfaceModifierKind> {
        if keyword == KeywordId::Async {
            return Some(SurfaceModifierKind::PrefixMarker);
        }
        None
    }

    /// Classify known decorators and provider-owned standard decorator paths.
    fn decorator_feature_for_path(&self, resolved_path: &[String]) -> Option<DecoratorFeature> {
        if let Some(id) = decorators::from_segments(resolved_path) {
            return Some(decorator_feature_from_id(id));
        }
        // Provider catalogs validate whether the module and function exist. The semantics pack only classifies
        // the shape here so a newly published `std.*` provider decorator does not need a compiler registry entry.
        if resolved_path.len() >= 3 && resolved_path[0] == stdlib::STDLIB_ROOT {
            return Some(DecoratorFeature::StdlibDecoratorFunction);
        }
        None
    }

    /// Resolve an assertion's checked shape to its standard testing operation.
    fn assert_call_target(&self, shape: AssertShape) -> Option<SurfaceCallTarget> {
        let local_name = match shape {
            AssertShape::Condition => "assert",
            AssertShape::Eq => "assert_eq",
            AssertShape::Ne => "assert_ne",
            AssertShape::Not => "assert_false",
        };
        Some(SurfaceCallTarget {
            local_name,
            canonical_path: vec![
                stdlib::STDLIB_ROOT.to_string(),
                "testing".to_string(),
                local_name.to_string(),
            ],
        })
    }

    // ---- Typechecker dispatch ----

    /// Route assertion statements to their checker action.
    fn typecheck_surface_stmt_action(&self, key: &SurfaceFeatureKey) -> Option<SurfaceStmtTypeCheck> {
        if matches!(key, SurfaceFeatureKey::SoftKeyword(KeywordId::Assert)) {
            return Some(SurfaceStmtTypeCheck::AssertCheck);
        }
        None
    }

    /// Route stdlib-owned surface expressions to their typechecker actions.
    fn typecheck_surface_expr_action(&self, key: &SurfaceFeatureKey) -> Option<SurfaceExprTypeCheck> {
        if matches!(key, SurfaceFeatureKey::SoftKeyword(KeywordId::Await)) {
            return Some(SurfaceExprTypeCheck::AwaitCheck);
        }
        if is_std_async_race_for_surface(key) {
            return Some(SurfaceExprTypeCheck::RaceForCheck);
        }
        None
    }

    /// Route `async` declaration modifiers to async callable validation.
    fn typecheck_surface_modifier_action(&self, key: &SurfaceFeatureKey) -> Option<SurfaceModifierTypeCheck> {
        if matches!(key, SurfaceFeatureKey::SoftKeyword(KeywordId::Async)) {
            return Some(SurfaceModifierTypeCheck::AsyncCallable);
        }
        None
    }

    // ---- Lowering dispatch ----

    /// Route checked assertion statements to their lowering action.
    fn lower_surface_stmt_action(&self, key: &SurfaceFeatureKey) -> Option<SurfaceStmtLoweringAction> {
        if matches!(key, SurfaceFeatureKey::SoftKeyword(KeywordId::Assert)) {
            return Some(SurfaceStmtLoweringAction::AssertCall);
        }
        None
    }

    /// Route stdlib-owned surface expressions to their lowering actions.
    fn lower_surface_expr_action(&self, key: &SurfaceFeatureKey) -> Option<SurfaceExprLoweringAction> {
        if matches!(key, SurfaceFeatureKey::SoftKeyword(KeywordId::Await)) {
            return Some(SurfaceExprLoweringAction::Await);
        }
        if is_std_async_race_for_surface(key) {
            return Some(SurfaceExprLoweringAction::RaceFor);
        }
        None
    }

    // ---- Runtime requirement scanning ----

    /// Require the async runtime for checked async and await constructs.
    fn modifier_runtime_requirement(&self, key: &SurfaceFeatureKey) -> RuntimeRequirement {
        if matches!(
            key,
            SurfaceFeatureKey::SoftKeyword(KeywordId::Async) | SurfaceFeatureKey::SoftKeyword(KeywordId::Await)
        ) {
            return RuntimeRequirement::AsyncRuntime;
        }
        RuntimeRequirement::None
    }

    /// Require the async runtime when the async standard module is imported.
    fn import_runtime_requirement(&self, module: &str) -> RuntimeRequirement {
        if module == "async" {
            return RuntimeRequirement::AsyncRuntime;
        }
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
