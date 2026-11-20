//! The lowering from checked Body IR to native plans for the rustc driver.
//!
//! The driver hands every checked module of a unit, with its source provenance, to [`unit_plan`]. Lowering answers a
//! complete [`Plan`], a refusal naming the construct it cannot lower, or the runtime declarations it still needs; the
//! driver resolves those and resumes with [`resume_unit_plan`]. The answers are byte-identical to the reference
//! toolchain's on the golden corpus ([`corpus`]).

use std::collections::BTreeMap;

use incan_semantics_core::body_ir::{BodyIrModule, NativeUnionDeclaration};
use incan_semantics_core::{CanonicalSymbolId, CompilerNodeId};

pub mod corpus;

mod body_index;
mod class_inheritance;
mod compiler_record_adapters;
mod continuation_context;
mod enum_instances;
mod native_type_identity;
mod nominal_frames;
mod nominal_instances;
mod parallel_ranges;
mod runtime_requirements;
mod symbols;
mod trace_channel;
mod type_parameters;

pub use incan_mir_lowering::{Plan, PlanType, SourceSpan};

/// Source provenance, collected tests and the unit's integer overflow policy, aligned with the module list.
#[derive(Clone, Debug, Default)]
pub struct SourceInputs {
    pub sources: Vec<String>,
    pub files: Vec<String>,
    pub tests: Vec<String>,
    pub package_versions: BTreeMap<String, String>,
    /// Ordinary integer operations follow the selected unit's profile; explicit helpers keep their own policies.
    pub overflow_checks: bool,
    /// Producer-owned checked inputs and consumed signatures; absence preserves ordinary lowering.
    pub scalar_products: Option<String>,
    pub scalar_bodies: Vec<(CompilerNodeId, String)>,
    /// Request-local producer evidence; absence keeps ordinary lowering and emits no report files.
    pub record_report: Option<String>,
}

/// An authenticated runtime declaration needed at an existing source operation's location.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeRequirement {
    /// None requests the independently addressed facade before its canonical declaration can be selected.
    pub canonical: Option<CanonicalSymbolId>,
    pub path: String,
    pub span: SourceSpan,
}

/// Lowering work retained while the driver resolves the runtime declarations that work selected.
pub struct UnitContinuation {
    _private: (),
}

/// A complete native plan, or retained work awaiting exact runtime declarations.
pub enum UnitPlanning {
    Complete(Option<Plan>),
    NeedsRuntime(UnitContinuation, Vec<RuntimeRequirement>),
}

/// A complete diagnostic census, or prerequisites that must resolve before it can publish its report.
pub enum RefusalPlanning {
    Complete(Vec<String>),
    NeedsRuntime(Vec<RuntimeRequirement>),
}

const UNIMPLEMENTED: &str = "unsupported lowering: the Rust lowering has not reached this unit";

/// Lower one checked program, library or collected test unit.
pub fn unit_plan(
    modules: &[BodyIrModule],
    inputs: &SourceInputs,
    entry: i64,
    kind: &str,
    published: &mut Vec<NativeUnionDeclaration>,
) -> Result<UnitPlanning, String> {
    let _ = (modules, inputs, entry, kind, published);
    Err(UNIMPLEMENTED.to_owned())
}

/// Resume retained work against the exact runtime declarations the driver resolved.
pub fn resume_unit_plan(
    continuation: UnitContinuation,
    modules: &[BodyIrModule],
    inputs: &SourceInputs,
    entry: i64,
    kind: &str,
    published: &mut Vec<NativeUnionDeclaration>,
) -> Result<UnitPlanning, String> {
    let _ = (continuation, modules, inputs, entry, kind, published);
    Err(UNIMPLEMENTED.to_owned())
}

/// Collect every body's first refusal, or all of one selected body's, for the refusal census.
pub fn program_refusals(
    modules: &[BodyIrModule],
    inputs: &SourceInputs,
    entry: i64,
    progress: bool,
    selection: &str,
    kind: &str,
) -> RefusalPlanning {
    let _ = (modules, inputs, entry, progress, selection, kind);
    RefusalPlanning::Complete(vec![UNIMPLEMENTED.to_owned()])
}

/// The runtime layouts a unit needs installed before its first lowering attempt.
pub fn layout_runtime_requirements(modules: &[BodyIrModule], file: String) -> Vec<RuntimeRequirement> {
    let _ = (modules, file);
    Vec::new()
}
