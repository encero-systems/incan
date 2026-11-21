//! Call argument storage: the parameter representation each frame slot holds and each call materializes.

use incan_mir_plan_native::{Constant, Operand, PlanType, RvalueKind, SourceSpan, assign as plan_assign};
use incan_semantics_core::IncanInputAccess;
use incan_semantics_core::body_ir::{
    ArgumentBinding, ArgumentElement, Body, BoundArgument, CallableParam, Operand as SourceOperand,
};

use crate::builtin_calls::coerce_numeric_operand;
use crate::carrier_operands::boundary_operand;
use crate::checked_values::scalar_place;
use crate::closures::{packed_input, parameter_representation};
use crate::collections::is_collection;
use crate::element_places::is_element_place;
use crate::explicit_arguments::argument_access;
use crate::formatting::display_operand;
use crate::frame::{Builder, literal, place_type, read_place, temporary};
use crate::generators::is_generator_reference;
use crate::generators::is_generator_type;
use crate::layout_types::native_source_type;
use crate::list_values::value_operand;
use crate::lists::exclusive_reference;
use crate::lists::is_list;
use crate::operand_borrows::{borrow_str_operand, borrow_text_operand};
use crate::pending::untranslated;
use crate::statements::is_nominal_type;

/// A deferred producer owns its value parameters; ordinary frames retain the shared collection ABI.
///
/// Call materialization, frame locals and constructor signatures use this same storage decision so an escaping
/// generator never captures a borrow of a caller's list snapshot. Explicit inputs select their checked access first;
/// deferred producers still refuse that contract until retained dependencies are represented. Inferred shared access
/// arrives at a type with a shared executable form for a private callable, and at any read-only type for a published
/// callable, as on the base; lowering lends both through the same carriers.
pub fn stored_parameter_type(body: &Body, parameter: &CallableParam, ty: PlanType) -> Result<PlanType, String> {
    if parameter.input_access != IncanInputAccess::Inferred {
        if body.is_generator() {
            return Err("unsupported Body IR explicit input access in a deferred producer".into());
        }
        return packed_input(
            ty,
            parameter.kind,
            parameter.mutable,
            parameter.native_owned,
            parameter.input_access,
        );
    }
    if body.is_generator() && !parameter.mutable {
        return Ok(ty);
    }
    packed_input(
        ty,
        parameter.kind,
        parameter.mutable,
        parameter.native_owned,
        parameter.input_access,
    )
}

/// Report whether a value that owns storage has no exclusive reference a `mut` parameter could receive.
pub fn lacks_exclusive_parameter(ty: &PlanType) -> bool {
    if !is_nominal_type(ty) || is_generator_type(ty) {
        return false;
    }
    exclusive_reference(ty).is_none()
}

/// Whether a parameter takes a list, set or dictionary by value rather than through a borrow.
pub fn owned_collection(ty: &PlanType) -> bool {
    matches!(ty, PlanType::List(..) | PlanType::Set(_) | PlanType::Dict(..))
}

/// The representation a call passes for one declared parameter, shared by functions and methods.
///
/// A list, set or dictionary parameter takes a shared or, when `mut`, an exclusive reference to the caller's storage;
/// `mut` text, models and enums take an exclusive reference to the caller's value; a `mut` scalar is the callee's own
/// copy. Rest parameters consume their checked owned packs. A `mut` tuple borrows its complete layout; an iterator
/// without retained exclusive storage stays refused.
pub fn call_parameter_type(builder: &Builder, body: &Body, parameter: &CallableParam) -> Result<PlanType, String> {
    let declared = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &parameter.ty,
        &builder.intrinsics,
        &builder.file,
    )?;
    let ty = parameter_representation(body, parameter.local.index(), declared)?;
    if parameter.input_access == IncanInputAccess::Inferred && parameter.mutable && lacks_exclusive_parameter(&ty) {
        return Err("unsupported Body IR mutable tuple or iterator parameter".into());
    }
    stored_parameter_type(body, parameter, ty)
}

/// Report whether a resolved call leaves at least one declared slot to its default.
pub fn has_defaulted_slots(binding: &ArgumentBinding) -> bool {
    match binding {
        ArgumentBinding::Resolved { defaulted_slots, .. }
        | ArgumentBinding::ResolvedDeclarationOrder { defaulted_slots, .. } => !defaulted_slots.is_empty(),
        _ => false,
    }
}

/// How a call fills its declared slots: an offset past a separately lowered receiver, owned collection slots, a
/// positional reading of an unresolved binding, and the callee's source input accesses.
#[derive(Clone, Debug, Default)]
pub struct SlotPolicy {
    pub offset: usize,
    pub owned: bool,
    pub positional: bool,
    pub accesses: Vec<IncanInputAccess>,
}

/// Place the written arguments into the callee's declared parameter slots.
///
/// Body IR has already evaluated arguments in the binding's recorded order. Fixed named free calls and imported SDK
/// factories can retain declaration order; written-order operations keep their explicit policy. The binding says which
/// slot each materialized value fills. Defaults, missing slots, named or spread elements are refused. An offset of one
/// skips a method receiver, whose borrow is lowered separately and whose slot is absent from the ordinary argument
/// binding. With `owned`, a collection slot receives an owned value, as an SDK crate's compiled signature takes it.
///
/// With `positional`, an unresolved binding whose elements are exactly one plain value per declared slot fills the
/// slots in written order: without a name, a spread or an omitted slot, position is the only binding. An SDK crate
/// call to an overloaded function arrives this way, since the checker records no slots for an overload set.
pub fn ordered_slot_operands(
    builder: &mut Builder,
    binding: &ArgumentBinding,
    elements: &[ArgumentElement],
    types: &[PlanType],
    span: &SourceSpan,
    policy: &SlotPolicy,
) -> Result<Vec<Operand>, String> {
    if policy.offset > elements.len() {
        return Err("invalid Body IR argument offset".into());
    }
    let count = types.len();
    if !policy.accesses.is_empty() && policy.accesses.len() != count {
        return Err("invalid Body IR argument input access count".into());
    }
    match binding {
        ArgumentBinding::Resolved {
            arguments,
            defaulted_slots,
        } => resolved_slot_operands(builder, arguments, defaulted_slots.len(), elements, types, span, policy),
        ArgumentBinding::ResolvedDeclarationOrder {
            arguments,
            defaulted_slots,
        } => {
            for index in 1..arguments.len() {
                if arguments[index].slot <= arguments[index - 1].slot {
                    return Err("unsupported Body IR binding outside declaration order".into());
                }
            }
            resolved_slot_operands(builder, arguments, defaulted_slots.len(), elements, types, span, policy)
        }
        ArgumentBinding::UnresolvedPositional => {
            if !policy.positional || elements.len() - policy.offset != count {
                return Err("unsupported Body IR unresolved ArgumentBinding".into());
            }
            let mut operands = Vec::with_capacity(count);
            for index in 0..count {
                match &elements[index + policy.offset] {
                    ArgumentElement::One(value) => {
                        let access = argument_access(&policy.accesses, count, index)?;
                        operands.push(call_argument_operand(
                            builder,
                            value,
                            &types[index],
                            span,
                            policy.owned,
                            access,
                        )?);
                    }
                    _ => return Err("unsupported Body IR named or spread ArgumentElement".into()),
                }
            }
            Ok(operands)
        }
    }
}

/// Pass already evaluated resolved values by slot, preserving the ownership order their binding variant records.
fn resolved_slot_operands(
    builder: &mut Builder,
    bindings: &[BoundArgument],
    default_count: usize,
    elements: &[ArgumentElement],
    types: &[PlanType],
    span: &SourceSpan,
    policy: &SlotPolicy,
) -> Result<Vec<Operand>, String> {
    let count = types.len();
    if default_count != 0 || bindings.len() != elements.len() - policy.offset || bindings.len() != count {
        return Err("unsupported Body IR defaulted or incomplete ArgumentBinding".into());
    }
    let mut operands: Vec<Operand> = (0..count).map(|_| literal(Constant::Unit, span)).collect();
    let mut occupied: Vec<usize> = Vec::new();
    for (index, bound) in bindings.iter().enumerate() {
        let slot = bound.slot;
        if slot >= count || occupied.contains(&slot) {
            return Err("invalid Body IR argument slot map".into());
        }
        occupied.push(slot);
        match elements[index + policy.offset].as_one() {
            Some(value) => {
                let access = argument_access(&policy.accesses, count, slot)?;
                operands[slot] = call_argument_operand(builder, value, &types[slot], span, policy.owned, access)?;
            }
            None => return Err("unsupported Body IR named or spread ArgumentElement".into()),
        }
    }
    Ok(operands)
}

/// Honor explicit input authority before ordinary collection borrowing, callable adaptation, or boundary coercion.
pub fn call_argument_operand(
    builder: &mut Builder,
    value: &SourceOperand,
    ty: &PlanType,
    span: &SourceSpan,
    owned: bool,
    input_access: IncanInputAccess,
) -> Result<Operand, String> {
    let _ = owned;
    if input_access != IncanInputAccess::Inferred {
        return Err(untranslated("call_arguments::explicit_argument_operand"));
    }
    match ty {
        PlanType::ClosureRef(_) | PlanType::BorrowingClosure(_) | PlanType::BorrowingClosureRef(_) => {
            return Err(untranslated("callable_calls::callable_argument"));
        }
        PlanType::StringRef => return borrow_text_operand(builder, value, span),
        PlanType::StrRef => return borrow_str_operand(builder, value, span),
        PlanType::ValidationErrorRef => return Err(untranslated("validation_errors::validation_error_reference")),
        PlanType::SliceRef(_) | PlanType::SliceMutRef(_) => {
            return Err(untranslated("operand_borrows::borrow_slice_operand"));
        }
        PlanType::ScalarRef(_) | PlanType::ScalarMutRef(_) => {
            return Err(untranslated("operand_borrows::borrow_scalar_operand"));
        }
        PlanType::FrozenCollectionRef(..) => return Err(untranslated("frozen_plan::borrow_frozen")),
        _ => {}
    }
    if owned_collection(ty) {
        // The checked ABI distinguishes owned packs and SDK values from ordinary collection references.
        return value_operand(builder, value, span);
    }
    if is_list(ty) || is_collection(ty) {
        return Err(untranslated("list_values::checked_list_argument"));
    }
    if is_generator_reference(ty) {
        return Err(untranslated("generators::generator_reference"));
    }
    if *ty == PlanType::String {
        let text = display_operand(builder, value, span)?;
        return Ok(read_place(text, true, span));
    }
    match ty {
        PlanType::ModelRef(..) | PlanType::EnumRef(..) | PlanType::TupleRef(_) => {
            return shared_nominal_argument(builder, value, ty, span);
        }
        PlanType::StringMutRef | PlanType::ModelMutRef(..) | PlanType::EnumMutRef(..) | PlanType::TupleMutRef(_) => {
            return Err(untranslated("call_arguments::mutable_argument"));
        }
        _ => {}
    }
    let operand = boundary_operand(builder, value, ty, span)?;
    let coerced = coerce_numeric_operand(builder, operand, ty, span)?;
    if let PlanType::FunctionPointer(_) = ty {
        return Err(untranslated("closures::function_assignment"));
    }
    Ok(coerced)
}

/// Borrow a checked nominal or enum, preserving shared list element loans and existing frame references.
pub fn shared_nominal_argument(
    builder: &mut Builder,
    value: &SourceOperand,
    ty: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let SourceOperand::Place(read) = value else {
        return Err("unsupported Body IR shared receiver without a place".into());
    };
    if is_element_place(builder, &read.place)? {
        return Err(untranslated("model_values::materialized_receiver"));
    }
    let receiver = scalar_place(builder, &read.place, span)?;
    let source = place_type(&builder.locals, &receiver);
    if source == *ty {
        return Ok(read_place(receiver, false, span));
    }
    match &source {
        PlanType::Enum(owner, name) => {
            if *ty != PlanType::EnumRef(*owner, name.clone()) {
                return Err(format!("unsupported Body IR shared enum owner mismatch for {name}"));
            }
        }
        PlanType::Tuple(elements) => {
            if *ty != PlanType::TupleRef(elements.clone()) {
                return Err("unsupported Body IR shared tuple owner mismatch".into());
            }
        }
        PlanType::Model(owner, name) => {
            if *ty != PlanType::ModelRef(*owner, name.clone()) {
                return Err(format!(
                    "unsupported Body IR shared receiver owner mismatch: {source:?} requires {ty:?} in {} at {}:{}",
                    builder.name, span.file, span.line
                ));
            }
        }
        _ => return Err("unsupported Body IR shared receiver outside its nominal frame".into()),
    }
    let reference = temporary(builder, ty.clone(), span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(receiver), span));
    Ok(read_place(reference, false, span))
}
