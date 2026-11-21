//! The lowering from checked Body IR to native plans for the rustc driver.
//!
//! The driver hands every checked module of a unit, with its source provenance, to [`unit_plan`]. Lowering answers a
//! complete [`Plan`], a refusal naming the construct it cannot lower, or the runtime declarations it still needs; the
//! driver resolves those and resumes with [`resume_unit_plan`]. The answers are byte-identical to the reference
//! toolchain's on the golden corpus ([`corpus`]).

use std::collections::BTreeMap;

use incan_semantics_core::CompilerNodeId;
use incan_semantics_core::body_ir::{BodyIrModule, NativeUnionDeclaration};

pub mod corpus;

mod assignments;
mod async_;
mod attributes;
mod body_index;
mod builtin_calls;
mod call_arguments;
mod callable_calls;
mod callable_methods;
mod calls;
mod carrier_operands;
mod checked_json;
mod checked_json_contracts;
mod checked_values;
mod class_inheritance;
mod classes;
mod closures;
mod collections;
mod comparison_methods;
mod compiler_record_adapters;
mod computed_statics;
mod continuation_context;
mod decls;
mod decorator_caches;
mod defaults;
mod display_methods;
mod element_borrows;
mod element_places;
mod enum_instances;
mod enums;
mod explicit_arguments;
mod fallible_factories;
mod fallible_methods;
mod frame;
mod frame_instances;
mod function_frames;
mod generators;
mod generic_enums;
mod generic_layouts;
mod instance_products;
mod iterator_methods;
mod layout_types;
mod library_model_arguments;
mod list_iteration;
mod list_values;
mod lists;
mod loop_exits;
mod lowering;
mod marker_traits;
mod matches;
mod model_lists;
mod models;
mod native_arguments;
mod native_traits;
mod native_type_identity;
mod nominal_frames;
mod nominal_instances;
mod numerics;
mod operand_borrows;
mod options;
mod pair_generators;
mod parallel_ranges;
mod pending;
mod plan_product_inputs;
mod program_context;
mod program_entries;
mod project;
mod ranges;
mod redo_counts;
mod reflection_carriers;
mod result_carriers;
mod result_requirements;
mod runtime_calls;
mod runtime_layouts;
mod runtime_requirements;
mod scalar_reads;
mod statements;
mod static_storage;
mod stored_types;
mod string_conversion;
mod string_conversion_contracts;
mod symbols;
mod text_plan;
mod trace_channel;
mod trait_links;
mod traits;
mod tuple_iterators;
mod tuples;
mod type_parameters;
mod types;
mod unions;

pub use incan_mir_plan_native::{Plan, PlanType, SourceSpan};
pub use lowering::{UnitContinuation, UnitPlanning};
pub use runtime_requirements::{RefusalPlanning, RuntimeRequirement};

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

impl SourceInputs {
    fn unit<'a>(&'a self, entry: i64, kind: &'a str) -> lowering::UnitInputs<'a> {
        lowering::UnitInputs {
            sources: &self.sources,
            files: &self.files,
            entry,
            kind,
            tests: &self.tests,
            package_versions: &self.package_versions,
            overflow_checks: self.overflow_checks,
            scalar_products: self.scalar_products.clone(),
            scalar_bodies: &self.scalar_bodies,
            record_report: self.record_report.clone(),
        }
    }
}

/// Lower one checked program, library or collected test unit.
pub fn unit_plan(
    modules: &[BodyIrModule],
    inputs: &SourceInputs,
    entry: i64,
    kind: &str,
    published: &mut Vec<NativeUnionDeclaration>,
) -> Result<UnitPlanning, String> {
    lowering::unit_plan(modules, &inputs.unit(entry, kind), published)
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
    let mut unit = inputs.unit(entry, kind);
    // A resumed attempt never reads keyed products again: its frames were captured by the first attempt.
    unit.scalar_products = None;
    unit.record_report = None;
    lowering::resume_unit_plan(continuation, modules, &unit, published)
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
    RefusalPlanning::Complete(vec![pending::untranslated("program_refusals::program_refusals")])
}

/// The runtime layouts a unit needs installed before its first lowering attempt.
pub fn layout_runtime_requirements(modules: &[BodyIrModule], file: String) -> Vec<RuntimeRequirement> {
    runtime_calls::layout_runtime_requirements(modules, &file)
}

/// Whether a plan type is an ordinary or sized numeric carrier, shared with native owned-result validation.
pub fn numeric_carrier(ty: &PlanType) -> bool {
    numerics::numeric_carrier(ty)
}
