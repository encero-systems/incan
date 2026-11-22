//! Indexed scalar reads borrow intermediate storage instead of copying its unrelated fields.

use std::sync::Arc;

use incan_mir_plan_native::{
    CalleeKind, ListLeaf, Local, OperandKind, PlanType, SourceSpan, TerminatorKind, TupleElement, source_span,
};
use incan_semantics_core::body_ir::{
    BodyIrModule, Constant as CheckedConstant, LocalId, Operand as CheckedOperand, OwnershipFact,
    Place as CheckedPlace, PlaceElem,
};

use crate::checked_values::place_read;
use crate::frame::{Builder, place_type};
use crate::native_type_identity::constructor_index;
use crate::program_context::synthetic_program_context;
use crate::string_input::{SyntheticFrame, synthetic_builder};

/// An isolated checked input frame; slot zero is the result and slot one is caller-owned storage.
fn indexed_frame(ty: PlanType, span: &SourceSpan) -> Builder {
    let module = BodyIrModule::declarations_only("indexed_field_reads".into());
    let constructors = Arc::new(constructor_index(&module.nominal_declarations));
    let mut builder = synthetic_builder(
        synthetic_program_context(&module, constructors),
        SyntheticFrame {
            locals: vec![
                Local {
                    ty: PlanType::Int,
                    span: span.clone(),
                },
                Local { ty, span: span.clone() },
            ],
            parameter_count: 1,
            file: "indexed_field_reads.incn".into(),
            module_identity: "indexed_field_reads".into(),
            unions: Vec::new(),
            intrinsics: Vec::new(),
            name: "indexed_read".into(),
            owned: Vec::new(),
        },
    );
    let _ = builder.new_block(span);
    builder
}

/// The checked read `local0[0].<field>`.
fn indexed_field(field: &str) -> CheckedPlace {
    let mut source = CheckedPlace::from_local(LocalId(0));
    source
        .projection
        .push(PlaceElem::Index(Box::new(CheckedOperand::Constant(
            CheckedConstant::Int(0),
        ))));
    source.projection.push(PlaceElem::structural_field(field));
    source
}

/// The native plan uses a shared bounds-checked lookup, then copies only the selected scalar or frozen handle.
#[test]
fn indexed_scalar_field_never_materializes_the_element() -> Result<(), String> {
    let frozen_list = || ("FrozenList".to_owned(), vec![PlanType::Int]);
    let cases = [
        (TupleElement::Int, PlanType::Int),
        (TupleElement::FrozenStr, PlanType::FrozenStr),
        (TupleElement::FrozenBytes, PlanType::FrozenBytes),
        (TupleElement::StaticText, PlanType::StaticText),
        (TupleElement::StaticBytes, PlanType::StaticBytes),
        (
            TupleElement::FrozenCollection(frozen_list().0, frozen_list().1),
            PlanType::FrozenCollection(frozen_list().0, frozen_list().1),
        ),
    ];
    for (field_type, expected) in cases {
        let span = source_span("indexed_field_reads.incn", 1, 1);
        let fields = vec![TupleElement::List(ListLeaf::Str, 1), field_type];
        let mut builder = indexed_frame(PlanType::ListRef(ListLeaf::Tuple(fields.clone()), 1), &span);
        let result = place_read(&mut builder, &indexed_field("1"), OwnershipFact::Copy, &span)?;
        let OperandKind::Copy(selected) = &result.kind else {
            return Err("scalar field read did not copy only its selected handle".into());
        };
        assert!(place_type(&builder.locals, selected) == expected);
        let shared_reads = builder
            .blocks
            .iter()
            .filter(|block| {
                matches!(
                    &block.terminator.kind,
                    TerminatorKind::Call(callee, _, _, _, _)
                        if matches!(&callee.kind, CalleeKind::Instantiated(name, _)
                            if name == "incan_std_primitives::collections::list_get")
                )
            })
            .count();
        assert_eq!(shared_reads, 1);
        for local in &builder.locals {
            assert!(local.ty != PlanType::Tuple(fields.clone()));
        }
    }
    Ok(())
}

/// An independently owned list result retains its clone operation without copying the containing tuple.
#[test]
fn indexed_owned_field_clones_only_the_selected_field() -> Result<(), String> {
    let span = source_span("indexed_field_reads.incn", 2, 1);
    let fields = vec![TupleElement::List(ListLeaf::Str, 1), TupleElement::Int];
    let mut builder = indexed_frame(PlanType::ListRef(ListLeaf::Tuple(fields.clone()), 1), &span);
    let result = place_read(&mut builder, &indexed_field("0"), OwnershipFact::Clone, &span)?;
    let OperandKind::Move(selected) = &result.kind else {
        return Err("owned field read did not return its independent list".into());
    };
    assert!(place_type(&builder.locals, selected) == PlanType::List(ListLeaf::Str, 1));
    for local in &builder.locals {
        assert!(local.ty != PlanType::Tuple(fields.clone()));
    }
    Ok(())
}
