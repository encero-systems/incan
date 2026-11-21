//! Cleanup lowering shares exact states and their predecessor tails without changing drop order.

use std::sync::Arc;

use incan_mir_plan_native::{Local, PlanType, Projection, TerminatorKind, TupleElement, Unwind, source_span};
use incan_semantics_core::body_ir::BodyIrModule;

use crate::frame::{Builder, cleanup_exit};
use crate::native_type_identity::ConstructorIndex;
use crate::program_context::synthetic_program_context;
use crate::string_input::{SyntheticFrame, synthetic_builder};

/// A frame whose eight owned strings expose cleanup-prefix duplication directly.
fn cleanup_builder() -> Builder {
    let span = source_span("cleanup_chains.incn", 1, 1);
    let mut locals = vec![Local {
        ty: PlanType::Unit,
        span: span.clone(),
    }];
    for _ in 0..8 {
        locals.push(Local {
            ty: PlanType::String,
            span: span.clone(),
        });
    }
    let module = BodyIrModule::declarations_only("test_cleanup_chains".into());
    let program = synthetic_program_context(&module, Arc::new(ConstructorIndex::default()));
    synthetic_builder(
        program,
        SyntheticFrame {
            locals,
            parameter_count: 0,
            file: span.file.to_string(),
            module_identity: "test_cleanup_chains".into(),
            unions: Vec::new(),
            intrinsics: Vec::new(),
            name: "test_cleanup_chains".into(),
            owned: Vec::new(),
        },
    )
}

/// The drop a block ends in: its place's local and projection, its target and its unwind action.
fn drop_of(builder: &Builder, block: i64) -> Option<(i64, Projection, i64, Unwind)> {
    match &builder.blocks[block as usize].terminator.kind {
        TerminatorKind::Drop(place, target, action) => {
            Some((place.local, place.projection.clone(), *target, action.clone()))
        }
        _ => None,
    }
}

/// Growing owner states share tails while a partially moved tuple keeps reverse initialized-field order.
#[test]
fn cleanup_chains_share_predecessor_tails() {
    let span = source_span("cleanup_chains.incn", 1, 1);
    let mut builder = cleanup_builder();
    let mut final_root = -1;
    for owner in 1..9 {
        builder.owned.push(owner);
        final_root = cleanup_exit(&mut builder, &span);
        for _ in 0..8 {
            assert_eq!(cleanup_exit(&mut builder, &span), final_root);
        }
    }
    assert_eq!(builder.blocks.len(), 9);
    assert_eq!(builder.cleanup_chains.len(), 9);

    let mut current = final_root;
    for expected in (1..9).rev() {
        let Some((local, projection, target, action)) = drop_of(&builder, current) else {
            panic!("block {current} does not drop owner {expected}");
        };
        assert_eq!(local, expected);
        assert!(projection == Projection::Whole);
        assert!(action == Unwind::Terminate);
        current = target;
    }
    assert!(builder.blocks[current as usize].terminator.kind == TerminatorKind::UnwindResume);

    builder.locals.push(Local {
        ty: PlanType::Tuple(vec![TupleElement::String, TupleElement::String, TupleElement::String]),
        span: span.clone(),
    });
    builder.owned.push(9);
    builder.moved_tuple_fields.push((9, 1));
    let partial_root = cleanup_exit(&mut builder, &span);
    for _ in 0..8 {
        assert_eq!(cleanup_exit(&mut builder, &span), partial_root);
    }
    assert_eq!(builder.blocks.len(), 11);
    assert_eq!(builder.cleanup_chains.len(), 10);
    let Some((local, projection, target, action)) = drop_of(&builder, partial_root) else {
        panic!("the partial root does not drop the tuple");
    };
    assert_eq!(local, 9);
    assert!(projection == Projection::Field(2, PlanType::String));
    assert!(action == Unwind::Terminate);
    let Some((local, projection, target, action)) = drop_of(&builder, target) else {
        panic!("the partial chain does not drop the tuple's first field");
    };
    assert_eq!(local, 9);
    assert!(projection == Projection::Field(0, PlanType::String));
    assert!(action == Unwind::Terminate);
    assert_eq!(target, final_root);
}
