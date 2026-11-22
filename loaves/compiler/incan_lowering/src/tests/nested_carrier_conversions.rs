//! A carrier held in another carrier's payload converts where it lives, and `?` converts the payload it keeps.
//!
//! A Rust call's result keeps its native storage, `Result[Option[list[u8]], str]`, while the checked source reads it
//! as `Result[Option[bytes], str]`: the two `Option` spellings are distinct carriers with the same payload layout, so
//! the inner `Option` converts at the boundary where the source reads it.

use std::sync::Arc;

use incan_mir_plan_native::{
    Carrier, EnumDeclaration, EnumVariant, FieldProjection, ListLeaf, Local, OperandKind, Place, PlanType, Projection,
    RvalueKind, StatementKind, TerminatorKind, carrier_index, carrier_position, source_span,
};
use incan_semantics_core::body_ir::BodyIrModule;

use crate::carrier_operands::boundary_plan_operand;
use crate::frame::{Builder, read_place, value_type};
use crate::native_type_identity::ConstructorIndex;
use crate::options::propagate_result;
use crate::program_context::synthetic_program_context;
use crate::string_input::{SyntheticFrame, synthetic_builder};

/// The native storage `Option` (carrier 0), the checked `Option[bytes]` (carrier 1), the native `Result` holding
/// carrier 0 (carrier 2) and the frame's own `Result[None, str]` (carrier 3).
const NATIVE_OPTION: i64 = 0;
const SOURCE_OPTION: i64 = 1;
const NATIVE_RESULT: i64 = 2;
const RETURNED_RESULT: i64 = 3;

fn bytes() -> PlanType {
    PlanType::List(ListLeaf::U8, 1)
}

fn carrier_type(position: i64) -> PlanType {
    PlanType::Enum(carrier_index(position), format!("__IncanCarrier{position}"))
}

fn carrier(position: i64, kind: Carrier, variants: [(&str, Vec<PlanType>); 2]) -> EnumDeclaration {
    let variants = variants
        .into_iter()
        .map(|(name, fields)| EnumVariant {
            name: name.into(),
            fields,
        })
        .collect();
    let mut declaration = EnumDeclaration::new(
        format!("__IncanCarrier{position}"),
        false,
        variants,
        vec!["Clone".into()],
        source_span("nested_carriers.incn", 1, 1),
    );
    declaration.carrier = kind;
    declaration
}

/// A frame returning `Result[None, str]` whose local 1 owns the native `Result` and local 2 holds the checked
/// `Option[bytes]`.
fn nested_carrier_builder() -> Builder {
    let span = source_span("nested_carriers.incn", 1, 1);
    let intrinsics = vec![
        carrier(
            NATIVE_OPTION,
            Carrier::Option,
            [("None", Vec::new()), ("Some", vec![bytes()])],
        ),
        carrier(
            SOURCE_OPTION,
            Carrier::Option,
            [("None", Vec::new()), ("Some", vec![bytes()])],
        ),
        carrier(
            NATIVE_RESULT,
            Carrier::Result,
            [
                ("Ok", vec![carrier_type(NATIVE_OPTION)]),
                ("Err", vec![PlanType::String]),
            ],
        ),
        carrier(
            RETURNED_RESULT,
            Carrier::Result,
            [("Ok", vec![PlanType::Unit]), ("Err", vec![PlanType::String])],
        ),
    ];
    let locals = [RETURNED_RESULT, NATIVE_RESULT, SOURCE_OPTION]
        .into_iter()
        .map(|position| Local {
            ty: carrier_type(position),
            span: span.clone(),
        })
        .collect();
    let module = BodyIrModule::declarations_only("test_nested_carriers".into());
    let program = synthetic_program_context(&module, Arc::new(ConstructorIndex::default()));
    let mut builder = synthetic_builder(
        program,
        SyntheticFrame {
            locals,
            parameter_count: 0,
            file: span.file.to_string(),
            module_identity: "test_nested_carriers".into(),
            unions: Vec::new(),
            intrinsics,
            name: "test_nested_carriers".into(),
            owned: vec![1],
        },
    );
    builder.new_block(&span);
    builder
}

/// The type a place names, requiring every payload step to read the field its carrier declares there, as the plan
/// validator does for an enum payload projection.
fn projected_type(builder: &Builder, place: &Place) -> Result<PlanType, String> {
    let steps = match &place.projection {
        Projection::Whole => Vec::new(),
        Projection::VariantField(variant, slot, ty) => {
            let mut step = FieldProjection::new(*slot, ty.clone());
            step.variant = *variant;
            vec![step]
        }
        Projection::Fields(path) => path.clone(),
        other => return Err(format!("unexpected projection {other:?}")),
    };
    let mut ty = builder.locals[place.local as usize].ty.clone();
    for step in steps {
        let PlanType::Enum(owner, _) = &ty else {
            return Err(format!("a payload step of local {} reads through {ty:?}", place.local));
        };
        let declared = builder.intrinsics[carrier_position(*owner) as usize]
            .variants
            .get(step.variant as usize)
            .and_then(|variant| variant.fields.get(step.slot as usize))
            .ok_or_else(|| format!("local {} has no payload {}.{}", place.local, step.variant, step.slot))?;
        if *declared != step.ty {
            return Err(format!(
                "enum payload projection of local {}: declared {declared:?}, projected {:?}",
                place.local, step.ty
            ));
        }
        ty = step.ty;
    }
    Ok(ty)
}

/// Every place the frame's statements and branches read or write, each checked against its carrier's declaration.
fn check_places(builder: &Builder) -> Result<(), String> {
    let mut places = Vec::new();
    for block in &builder.blocks {
        for statement in &block.statements {
            let StatementKind::Assign(destination, value) = &statement.kind else {
                continue;
            };
            places.push(destination.clone());
            match &value.kind {
                RvalueKind::IsVariant(place, _) => places.push(place.clone()),
                RvalueKind::Use(operand) => places.extend(operand_place(operand)),
                RvalueKind::Enum(_, _, operands) => places.extend(operands.iter().filter_map(operand_place)),
                _ => {}
            }
        }
        if let TerminatorKind::SwitchBool(condition, ..) = &block.terminator.kind {
            places.extend(operand_place(condition));
        }
    }
    for place in &places {
        projected_type(builder, place)?;
    }
    Ok(())
}

fn operand_place(operand: &incan_mir_plan_native::Operand) -> Option<Place> {
    match &operand.kind {
        OperandKind::Copy(place) | OperandKind::Move(place) => Some(place.clone()),
        OperandKind::Literal(_) => None,
    }
}

/// The native `Option` in the native `Result`'s `Ok` payload, read where it lives.
fn native_payload() -> Place {
    let mut step = FieldProjection::new(0, carrier_type(NATIVE_OPTION));
    step.variant = 0;
    Place {
        local: 1,
        projection: Projection::Fields(vec![step]),
        span: source_span("nested_carriers.incn", 2, 5),
    }
}

/// Converting the `Option` held in a `Result` payload reads its `Some` payload through that payload, never as a
/// payload of the `Result` itself.
#[test]
fn a_carrier_in_a_payload_converts_where_it_lives() -> Result<(), String> {
    let span = source_span("nested_carriers.incn", 2, 5);
    let mut builder = nested_carrier_builder();
    let converted = boundary_plan_operand(
        &mut builder,
        read_place(native_payload(), true, &span),
        &carrier_type(SOURCE_OPTION),
        &span,
    )?;
    assert_eq!(value_type(&builder, &converted)?, carrier_type(SOURCE_OPTION));
    check_places(&builder)
}

/// `?` on the native `Result` stores its `Ok` payload as the checked `Option[bytes]` its destination holds.
#[test]
fn try_propagation_converts_the_payload_it_keeps() -> Result<(), String> {
    let span = source_span("nested_carriers.incn", 3, 5);
    let mut builder = nested_carrier_builder();
    let source = Place {
        local: 1,
        projection: Projection::Whole,
        span: span.clone(),
    };
    let destination = Place {
        local: 2,
        projection: Projection::Whole,
        span: span.clone(),
    };
    propagate_result(&mut builder, source, destination, &span)?;
    check_places(&builder)?;
    let stored = builder
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .filter_map(|statement| match &statement.kind {
            StatementKind::Assign(place, value) if place.local == 2 => Some(value),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [value] = stored.as_slice() else {
        return Err(format!(
            "expected one store into the destination, found {}",
            stored.len()
        ));
    };
    let RvalueKind::Use(operand) = &value.kind else {
        return Err(format!("the destination stores {:?}", value.kind));
    };
    assert_eq!(value_type(&builder, operand)?, carrier_type(SOURCE_OPTION));
    Ok(())
}
