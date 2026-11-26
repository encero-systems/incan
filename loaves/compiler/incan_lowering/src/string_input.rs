//! Owned String ingress frames shared by checked reconstruction and raw source conversions.

use std::sync::Arc;

use incan_mir_plan_native::{EnumDeclaration, Local, PlanType, SourceSpan};
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::{IncanInputAccess, IncanType};

use crate::frame::Builder;
use crate::native_type_identity::ConstructorIndex;
use crate::program_context::{ProgramContext, synthetic_program_context};

/// Keep the sole input owned until a planned call or construction consumes it, with one local cleanup ledger.
pub fn owned_string_frame(
    constructors: Arc<ConstructorIndex>,
    module: &BodyIrModule,
    name: &str,
    result: PlanType,
    intrinsics: &[EnumDeclaration],
    span: &SourceSpan,
) -> Builder {
    let frame = SyntheticFrame {
        locals: vec![
            Local {
                ty: result,
                span: span.clone(),
            },
            Local {
                ty: PlanType::String,
                span: span.clone(),
            },
        ],
        parameter_count: 1,
        file: span.file.to_string(),
        module_identity: module.module_id.path().to_string(),
        unions: Vec::new(),
        intrinsics: intrinsics.to_vec(),
        name: name.to_string(),
        owned: vec![1],
    };
    let mut builder = synthetic_builder(synthetic_program_context(module, constructors), frame);
    builder.new_block(span);
    builder
}

/// The storage a synthesized frame starts from; every other builder field starts empty.
pub struct SyntheticFrame {
    pub locals: Vec<Local>,
    pub parameter_count: i64,
    pub file: String,
    pub module_identity: String,
    pub unions: Vec<IncanType>,
    pub intrinsics: Vec<EnumDeclaration>,
    pub name: String,
    pub owned: Vec<i64>,
}

/// A builder for a frame the lowering synthesizes rather than reads from a checked body.
pub fn synthetic_builder(program: ProgramContext, frame: SyntheticFrame) -> Builder {
    Builder {
        program,
        locals: frame.locals,
        parameter_count: frame.parameter_count,
        blocks: Vec::new(),
        current: 0,
        source: String::new(),
        file: frame.file,
        line_starts: Vec::new(),
        module_identity: frame.module_identity,
        callable_identity: None,
        loops: Vec::new(),
        loop_owners: Vec::new(),
        externals: Vec::new(),
        owned: frame.owned,
        cleanup_chains: Vec::new(),
        closed: Vec::new(),
        iterators: Vec::new(),
        borrowed_items: Vec::new(),
        in_place_items: Vec::new(),
        partial_callables: Vec::new(),
        scalar_closures: Vec::new(),
        unproven_closures: Vec::new(),
        unions: frame.unions,
        local_offset: 0,
        consuming_closure_frame: false,
        intrinsics: frame.intrinsics,
        generator_handle: -1,
        instances: Vec::new(),
        type_parameters: Vec::new(),
        type_arguments: Vec::new(),
        callable_type_arguments: Vec::new(),
        trait_environment: None,
        function_pointer_locals: Vec::new(),
        name: frame.name,
        synthesized: Vec::new(),
        materialized_closures: Vec::new(),
        step_element: None,
        anchor: None,
        iteration_locals: Vec::new(),
        step_bindings: Vec::new(),
        step_clauses: Vec::new(),
        scoped_locals: Vec::new(),
        scope_parents: Vec::new(),
        loop_scopes: Vec::new(),
        loop_exit_blocks: Vec::new(),
        loop_exit_owners: Vec::new(),
        forbidden_reads: Vec::new(),
        moved_tuple_fields: Vec::new(),
        body_identity: None,
        checked_local_types: Vec::new(),
        initializing_decorator: None,
        cached_decorator: -1,
        callable_name_dynamic_profile: false,
        reference_values: Vec::new(),
        returned_view: IncanInputAccess::Inferred,
        carries_view: false,
        invocation_storage: Vec::new(),
        released: Vec::new(),
    }
}
