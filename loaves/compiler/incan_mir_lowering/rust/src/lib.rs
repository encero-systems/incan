//! Native invocation-span guards for the Incan-authored MIR lowering pipeline.
//!
//! Incan owns stage selection and guard lifetimes. This facet exists only because tracing callsites are Rust macros;
//! it does not own lowering orchestration or compiler behavior.

use tracing::span::EnteredSpan;

/// One entered lowering span that closes when Incan drops the guard at the exact stage boundary.
pub struct LoweringStage(#[allow(dead_code)] EnteredSpan);

/// Trace checked-project validation and construction of the shared lowering context.
pub fn context_preparation() -> LoweringStage {
    LoweringStage(
        tracing::trace_span!(target: "incan::invocation", "incan_mir_lowering::context_preparation").entered(),
    )
}

/// Trace declaration roots and computed static construction.
pub fn statics_and_roots() -> LoweringStage {
    LoweringStage(tracing::trace_span!(target: "incan::invocation", "incan_mir_lowering::statics_and_roots").entered())
}

/// Trace ordinary-frame lowering, excluding its deterministic parent merge.
pub fn ordinary_frames() -> LoweringStage {
    LoweringStage(tracing::trace_span!(target: "incan::invocation", "incan_mir_lowering::ordinary_frames").entered())
}

/// Trace the deterministic merge of independently lowered module ranges.
pub fn merge() -> LoweringStage {
    LoweringStage(tracing::trace_span!(target: "incan::invocation", "incan_mir_lowering::merge").entered())
}

/// Trace specialization, its executable instance queue and completed instance layouts.
pub fn specialization_and_instance_queue() -> LoweringStage {
    LoweringStage(
        tracing::trace_span!(target: "incan::invocation", "incan_mir_lowering::specialization_and_instance_queue")
            .entered(),
    )
}

/// Trace one continuation attempt after the frontend resolves a runtime declaration set.
pub fn runtime_resolution_resume() -> LoweringStage {
    LoweringStage(
        tracing::trace_span!(target: "incan::invocation", "incan_mir_lowering::runtime_resolution_resume").entered(),
    )
}

/// Trace final plan publication after all executable instances are closed.
pub fn plan_publication() -> LoweringStage {
    LoweringStage(tracing::trace_span!(target: "incan::invocation", "incan_mir_lowering::plan_publication").entered())
}
