//! Transient dependency requests produced by the runtime operations a planning attempt actually selects.

use incan_mir_plan_native::{SourceSpan, StaticCell};
use incan_semantics_core::CanonicalSymbolId;

/// An authenticated runtime declaration needed at an existing source operation's location.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeRequirement {
    /// None requests the independently addressed facade before its canonical declaration can be selected.
    pub canonical: Option<CanonicalSymbolId>,
    pub path: String,
    pub span: SourceSpan,
}

/// Runtime and instance retention completes, or asks for a missing authenticated declaration.
pub enum RuntimeRetention {
    Complete,
    NeedsRuntime(Vec<RuntimeRequirement>),
}

/// Static cells retained by completed frame planning, or a preflight invariant failure.
pub enum FramesPlanning {
    Complete(Vec<StaticCell>),
    NeedsRuntime(Vec<RuntimeRequirement>),
}

/// Completed body storage and its original nominal count, or a preflight invariant failure.
pub enum BodyPlanning {
    Complete((Vec<StaticCell>, usize)),
    NeedsRuntime(Vec<RuntimeRequirement>),
}

/// A complete diagnostic census, or prerequisites that must resolve before it can publish its report.
pub enum RefusalPlanning {
    Complete(Vec<String>),
    NeedsRuntime(Vec<RuntimeRequirement>),
}
