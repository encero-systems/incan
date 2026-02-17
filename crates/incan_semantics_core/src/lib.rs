//! Shared surface-semantics contracts for the Incan compiler.
//!
//! This crate defines the stable identity types ([`SurfaceFeatureKey`]), payload categories, and handler interfaces
//! ([`SurfaceSemanticsPack`], [`SurfaceSemanticsRegistry`]) that every compiler stage — parser, typechecker, lowering,
//! and emission — uses to route soft-keyword and decorator behavior.
//!
//! ## Why a separate crate?
//!
//! `incan_syntax` (parser/AST) and `incan_semantics_stdlib` (stdlib pack implementations) both need these types, but
//! they must not depend on each other. Extracting the contracts into a dependency-minimal crate lets the parser emit
//! generic `Surface` AST nodes tagged with [`SurfaceFeatureKey`] without knowing *which* stdlib features are enabled,
//! while the stdlib pack crate implements the feature logic without importing the parser.
//!
//! ## Extension model
//!
//! To support a new soft keyword or decorator family:
//!
//! 1. Add identity variants to [`SurfaceFeatureKey`] / [`DecoratorFeature`] as needed.
//! 2. Extend the payload-kind enums if the parser needs a new generic shape.
//! 3. Implement [`SurfaceSemanticsPack`] in a pack crate (see `incan_semantics_stdlib`).

use incan_core::lang::decorators::DecoratorId;
use incan_core::lang::keywords::KeywordId;

/// Stable feature key used by parser handoff and semantics dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SurfaceFeatureKey {
    SoftKeyword(KeywordId),
    Decorator(DecoratorFeature),
}

/// Canonical decorator-feature identities used by semantics packs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecoratorFeature {
    RustExtern,
    TestingMarker,
    Route,
    Derive,
    Requires,
    StdlibDecoratorFunction,
}

/// Generic parser payload category for surface statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceStmtPayloadKind {
    KeywordArgs,
}

/// Generic parser payload category for surface expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceExprPayloadKind {
    PrefixUnary,
}

/// Generic parser payload category for declaration modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceModifierKind {
    PrefixMarker,
}

/// Normalized assert condition shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssertShape {
    Condition,
    Eq,
    Ne,
    Not,
}

/// Canonical call target returned by lowering handlers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceCallTarget {
    pub local_name: &'static str,
    pub canonical_path: Vec<String>,
}

// ============================================================================
// Compiler-stage action descriptors
//
// These enums describe *what* the compiler should do, not *how*. The pack selects the action; the compiler core
// executes it. This keeps feature knowledge in the pack and execution knowledge in the compiler.
// ============================================================================

/// Describes how to lower a surface statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceStmtLoweringAction {
    /// Desugar keyword-args as an assert-pattern call (condition + optional message).
    ///
    /// The registry's [`SurfaceSemanticsPack::assert_call_target`] provides the actual call target; the compiler
    /// decomposes the condition shape and builds an IR call expression.
    AssertCall,
}

/// Describes how to lower a surface expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceExprLoweringAction {
    /// Lower prefix-unary payload as an await expression.
    Await,
}

/// Describes how to typecheck a surface statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceStmtTypeCheck {
    /// First keyword arg must be bool-compatible; optional second arg must be str-compatible.
    AssertCheck,
}

/// Describes how to typecheck a surface expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceExprTypeCheck {
    /// Must be in async context; inner expression type is returned.
    AwaitCheck,
}

/// Runtime requirement implied by a surface feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeRequirement {
    /// No special runtime needed.
    None,
    /// Async runtime (e.g., Tokio) is required.
    AsyncRuntime,
}

/// Semantics-pack contract.
///
/// Packs are intentionally pure and stateless; callers can construct a registry over enabled packs.
///
/// # Compiler-stage coverage
///
/// The trait covers every compiler stage so adding a new soft keyword is purely a pack concern:
///
/// | Stage        | Methods                                                             |
/// | ------------ | ------------------------------------------------------------------- |
/// | Parser       | `statement_payload_*`, `expression_payload_*`, `modifier_payload_*` |
/// | Typechecker  | `typecheck_surface_stmt_action`, `typecheck_surface_expr_action`    |
/// | Lowering     | `lower_surface_stmt_action`, `lower_surface_expr_action`            |
/// | Scanning     | `modifier_runtime_requirement`, `import_runtime_requirement`        |
/// | Call targets | `assert_call_target`                                                |
pub trait SurfaceSemanticsPack {
    // ---- Parser routing ----

    /// Return the parser payload kind for a soft-keyword statement, or `None` if not handled.
    fn statement_payload_for_soft_keyword(&self, _keyword: KeywordId) -> Option<SurfaceStmtPayloadKind> {
        None
    }

    /// Return the parser payload kind for a soft-keyword expression, or `None` if not handled.
    fn expression_payload_for_soft_keyword(&self, _keyword: KeywordId) -> Option<SurfaceExprPayloadKind> {
        None
    }

    /// Return the parser payload kind for a soft-keyword declaration modifier, or `None` if not handled.
    fn modifier_payload_for_soft_keyword(&self, _keyword: KeywordId) -> Option<SurfaceModifierKind> {
        None
    }

    /// Map a resolved decorator path to a canonical [`DecoratorFeature`], or `None` if not handled.
    fn decorator_feature_for_path(&self, _resolved_path: &[String]) -> Option<DecoratorFeature> {
        None
    }

    // ---- Typechecker dispatch ----

    /// Return the typecheck action for a surface statement, or `None` if not handled.
    fn typecheck_surface_stmt_action(&self, _key: &SurfaceFeatureKey) -> Option<SurfaceStmtTypeCheck> {
        None
    }

    /// Return the typecheck action for a surface expression, or `None` if not handled.
    fn typecheck_surface_expr_action(&self, _key: &SurfaceFeatureKey) -> Option<SurfaceExprTypeCheck> {
        None
    }

    // ---- Lowering dispatch ----

    /// Return the lowering action for a surface statement, or `None` if not handled.
    fn lower_surface_stmt_action(&self, _key: &SurfaceFeatureKey) -> Option<SurfaceStmtLoweringAction> {
        None
    }

    /// Return the lowering action for a surface expression, or `None` if not handled.
    fn lower_surface_expr_action(&self, _key: &SurfaceFeatureKey) -> Option<SurfaceExprLoweringAction> {
        None
    }

    // ---- Call targets ----

    /// Return the canonical call target for an assert-shaped condition, or `None` if not handled.
    fn assert_call_target(&self, _shape: AssertShape) -> Option<SurfaceCallTarget> {
        None
    }

    // ---- Runtime requirement scanning ----

    /// Runtime requirement implied by a declaration-level surface modifier.
    fn modifier_runtime_requirement(&self, _key: &SurfaceFeatureKey) -> RuntimeRequirement {
        RuntimeRequirement::None
    }

    /// Runtime requirement implied by importing a stdlib module (e.g., `"async"`).
    ///
    /// `module` is the second segment of a `std.<module>` import path.
    fn import_runtime_requirement(&self, _module: &str) -> RuntimeRequirement {
        RuntimeRequirement::None
    }
}

/// Registry for enabled semantics packs.
#[derive(Default)]
pub struct SurfaceSemanticsRegistry<'a> {
    packs: Vec<&'a dyn SurfaceSemanticsPack>,
}

impl<'a> SurfaceSemanticsRegistry<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_pack(mut self, pack: &'a dyn SurfaceSemanticsPack) -> Self {
        self.packs.push(pack);
        self
    }

    // ---- Parser routing ----

    pub fn statement_feature_for_soft_keyword(&self, keyword: KeywordId) -> Option<SurfaceFeatureKey> {
        self.packs.iter().find_map(|pack| {
            pack.statement_payload_for_soft_keyword(keyword)
                .map(|_| SurfaceFeatureKey::SoftKeyword(keyword))
        })
    }

    pub fn expression_feature_for_soft_keyword(&self, keyword: KeywordId) -> Option<SurfaceFeatureKey> {
        self.packs.iter().find_map(|pack| {
            pack.expression_payload_for_soft_keyword(keyword)
                .map(|_| SurfaceFeatureKey::SoftKeyword(keyword))
        })
    }

    pub fn modifier_feature_for_soft_keyword(&self, keyword: KeywordId) -> Option<SurfaceFeatureKey> {
        self.packs.iter().find_map(|pack| {
            pack.modifier_payload_for_soft_keyword(keyword)
                .map(|_| SurfaceFeatureKey::SoftKeyword(keyword))
        })
    }

    pub fn decorator_feature_for_path(&self, resolved_path: &[String]) -> Option<SurfaceFeatureKey> {
        self.packs.iter().find_map(|pack| {
            pack.decorator_feature_for_path(resolved_path)
                .map(SurfaceFeatureKey::Decorator)
        })
    }

    // ---- Typechecker dispatch ----

    pub fn typecheck_surface_stmt_action(&self, key: &SurfaceFeatureKey) -> Option<SurfaceStmtTypeCheck> {
        self.packs
            .iter()
            .find_map(|pack| pack.typecheck_surface_stmt_action(key))
    }

    pub fn typecheck_surface_expr_action(&self, key: &SurfaceFeatureKey) -> Option<SurfaceExprTypeCheck> {
        self.packs
            .iter()
            .find_map(|pack| pack.typecheck_surface_expr_action(key))
    }

    // ---- Lowering dispatch ----

    pub fn lower_surface_stmt_action(&self, key: &SurfaceFeatureKey) -> Option<SurfaceStmtLoweringAction> {
        self.packs.iter().find_map(|pack| pack.lower_surface_stmt_action(key))
    }

    pub fn lower_surface_expr_action(&self, key: &SurfaceFeatureKey) -> Option<SurfaceExprLoweringAction> {
        self.packs.iter().find_map(|pack| pack.lower_surface_expr_action(key))
    }

    // ---- Call targets ----

    pub fn assert_call_target(&self, shape: AssertShape) -> Option<SurfaceCallTarget> {
        self.packs.iter().find_map(|pack| pack.assert_call_target(shape))
    }

    // ---- Runtime requirement scanning ----

    pub fn modifier_runtime_requirement(&self, key: &SurfaceFeatureKey) -> RuntimeRequirement {
        for pack in &self.packs {
            let req = pack.modifier_runtime_requirement(key);
            if req != RuntimeRequirement::None {
                return req;
            }
        }
        RuntimeRequirement::None
    }

    pub fn import_runtime_requirement(&self, module: &str) -> RuntimeRequirement {
        for pack in &self.packs {
            let req = pack.import_runtime_requirement(module);
            if req != RuntimeRequirement::None {
                return req;
            }
        }
        RuntimeRequirement::None
    }
}

/// Map a core decorator id to canonical decorator feature.
pub fn decorator_feature_from_id(id: DecoratorId) -> DecoratorFeature {
    match id {
        DecoratorId::RustExtern => DecoratorFeature::RustExtern,
        DecoratorId::Route => DecoratorFeature::Route,
        DecoratorId::Derive => DecoratorFeature::Derive,
        DecoratorId::Requires => DecoratorFeature::Requires,
    }
}
