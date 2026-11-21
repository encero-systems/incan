//! Normalize checked primitive capability calls to their ordinary source-model implementations.

use incan_mir_plan_native::{PlanType, SourceSpan};
use incan_semantics_core::body_ir::{
    AggregateKind, ArgumentBinding, ArgumentElement, BodyIrModule, CallableTarget, Callee, ConstructorTarget, LocalId,
    MethodTarget, NamedCallableTarget, Operand as SourceOperand, OwnershipFact, Place as SourcePlace, Rvalue,
    Statement, StatementKind,
};
use incan_semantics_core::{CanonicalSymbolId, IncanType};

use crate::assignments::lower_assignment;
use crate::callable_calls::has_adapter_slot;
use crate::frame::{Builder, place_type, temporary};
use crate::layout_types::native_source_type;
use crate::list_methods::argument_place;
use crate::sequence_builtins::source_operand_type;
use crate::type_parameters::substitute;
use crate::value_enum_methods::{ordinal_value_owner, select_value_enum_raw, value_enum_raw_type};

/// Normalize a checked primitive capability call through its retained source implementation.
///
/// 1. Preserve the complete call record, including its destination, source span and panic fact.
/// 2. Normalize a primitive receiver or associated target through its checked adapter.
/// 3. Return the ordinary source call for the existing call dispatcher.
pub fn normalized_primitive_call(
    module: &BodyIrModule,
    builder: &mut Builder,
    statement: &Statement,
    span: &SourceSpan,
) -> Result<Option<Statement>, String> {
    // 1. Read the complete source call record without changing its destination or panic fact.
    let StatementKind::Call { callee, args, .. } = &statement.kind else {
        return Ok(None);
    };
    // 2. Normalize only the receiver or target selected by the checked adapter.
    match callee {
        Callee::Method(target) => {
            let Some(retained) = normalized_primitive_receiver(module, builder, target, args, span)? else {
                return Ok(None);
            };
            // 3. Preserve the ordinary call's frozen destination, source span and panic fact.
            Ok(statement.with_call(Callee::Method(target.clone()), retained))
        }
        Callee::Function(CallableTarget::Named(target)) => {
            let Some(retained) = normalized_primitive_static(module, builder, target)? else {
                return Ok(None);
            };
            let named = Callee::Function(CallableTarget::Named(retained));
            // 3. Preserve the ordinary call's frozen destination, source span and panic fact.
            Ok(statement.with_call(named, args.clone()))
        }
        _ => Ok(None),
    }
}

/// Wrap an admitted primitive receiver once before ordinary method dispatch.
///
/// 1. Select its retained source adapter by the exact checked slot and receiver carrier.
/// 2. Construct that model through its checked single-field layout.
/// 3. Retain every ordinary argument while borrowing the constructed adapter as the receiver.
fn normalized_primitive_receiver(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    span: &SourceSpan,
) -> Result<Option<Vec<ArgumentElement>>, String> {
    // 1. Select the retained source adapter for this exact slot and receiver.
    let retained_arguments = primitive_receiver_arguments(module, builder, target, arguments, span)?;
    let Some(identity) = &target.canonical else {
        return Ok(None);
    };
    if !has_adapter_slot(module, identity) {
        return Ok(None);
    }
    if arguments.is_empty() {
        return Err("invalid Body IR primitive capability without a receiver".into());
    }
    let receiver = retained_receiver_type(builder, &retained_arguments, span)?;
    let Some(owner) = adapter_owner(module, builder, identity, &receiver)? else {
        return Ok(None);
    };
    // 2. Construct the source model through its checked single-field layout.
    let declaration = &module.nominal_declarations[owner];
    let owner_type = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &IncanType::Named(declaration.name.clone()),
        &builder.intrinsics,
        "",
    )?;
    let storage = temporary(builder, owner_type, span);
    let destination = temporary_source_place(builder, storage.local)?;
    let constructor = ConstructorTarget {
        name: declaration.name.clone(),
        canonical: Some(declaration.canonical.clone()),
        direct_declaration_id: Some(declaration.direct_declaration_id.clone()),
        canonical_field_layout: Some(declaration.fields.clone()),
        binding: ArgumentBinding::resolved_positional(1),
    };
    let value = Rvalue::Aggregate(
        AggregateKind::Constructor(Box::new(constructor)),
        vec![retained_arguments[0].clone()],
    );
    lower_assignment(module, builder, &destination, &value, span)?;
    // 3. Preserve the ordinary arguments and borrow the source adapter.
    let receiver = SourceOperand::place(destination, OwnershipFact::Borrow, false);
    let mut retained = vec![ArgumentElement::One(receiver)];
    retained.extend(retained_arguments.into_iter().skip(1));
    Ok(Some(retained))
}

/// Read a materialized argument list through the ordinary checked argument-type boundary.
fn retained_receiver_type(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    span: &SourceSpan,
) -> Result<PlanType, String> {
    match arguments.first().and_then(ArgumentElement::as_one) {
        Some(value) => source_operand_type(builder, value, span),
        None => Err("unsupported Body IR primitive receiver spread".into()),
    }
}

/// Project a checked value enum's raw payload before using its source-owned primitive adapter.
fn primitive_receiver_arguments(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &MethodTarget,
    arguments: &[ArgumentElement],
    span: &SourceSpan,
) -> Result<Vec<ArgumentElement>, String> {
    let ordinal_bytes = target
        .canonical
        .as_ref()
        .is_some_and(|identity| has_adapter_slot(module, identity) && identity.declaration_name == "ordinal_bytes");
    if !ordinal_bytes || arguments.len() != 1 {
        return Ok(arguments.to_vec());
    }
    let source = argument_place(builder, arguments, 0, span)?;
    let Some(owner) = ordinal_value_owner(builder, &place_type(&builder.locals, &source)) else {
        return Ok(arguments.to_vec());
    };
    let raw_type = value_enum_raw_type(builder, owner);
    let raw = temporary(builder, raw_type.clone(), span);
    select_value_enum_raw(builder, owner, source, raw.clone(), span)?;
    let retained = temporary_source_place(builder, raw.local)?;
    // The getter created an owned text temporary; transfer it into the adapter's payload field.
    let ownership = if raw_type == PlanType::String {
        OwnershipFact::Move
    } else {
        OwnershipFact::Copy
    };
    Ok(vec![ArgumentElement::One(SourceOperand::place(
        retained, ownership, false,
    ))])
}

/// Select a source associated implementation without reconstructing any callable identity.
///
/// 1. Close the checked primitive receiver in the caller's existing binding.
/// 2. Select its exact source implementation or checked primitive decoder.
fn normalized_primitive_static(
    module: &BodyIrModule,
    builder: &Builder,
    target: &NamedCallableTarget,
) -> Result<Option<NamedCallableTarget>, String> {
    // 1. Close the checked primitive receiver in the caller's existing binding.
    let Some(identity) = &target.canonical else {
        return Ok(None);
    };
    if !has_adapter_slot(module, identity) {
        return Ok(None);
    }
    let Some(ty) = &target.receiver_type else {
        return Ok(None);
    };
    let receiver = substitute(ty, &builder.type_parameters, &builder.type_arguments)?;
    let concrete = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &receiver,
        &builder.intrinsics,
        "",
    )?;
    let Some(owner) = adapter_owner(module, builder, identity, &concrete)? else {
        return Ok(None);
    };
    // 2. Select its exact source implementation or checked primitive decoder.
    let declaration = &module.nominal_declarations[owner];
    let mut result = target.clone();
    result.receiver_type = Some(IncanType::Named(declaration.name.clone()));
    for adoption in &declaration.trait_adoptions {
        if let Some(adapter) = &adoption.primitive_adapter
            && *identity == adapter.decode_slot
        {
            result.canonical = Some(adapter.decoded_value.clone());
            return Ok(Some(result));
        }
    }
    let mut implementation: Option<&CanonicalSymbolId> = None;
    for record in &module.trait_implementations {
        if record.owner != declaration.canonical || record.method != *identity {
            continue;
        }
        if !record.slot_arguments.is_empty() || !record.type_arguments.is_empty() {
            return Err("unsupported Body IR generic primitive adapter associated implementation".into());
        }
        match implementation {
            Some(previous) if *previous != record.implementation => {
                return Err("unsupported Body IR conflicting primitive adapter associated implementations".into());
            }
            Some(_) => {}
            None => implementation = Some(&record.implementation),
        }
    }
    let Some(selected) = implementation else {
        return Err("unsupported Body IR primitive adapter without associated implementation".into());
    };
    result.canonical = Some(selected.clone());
    Ok(Some(result))
}

/// Select a unique checked adapter without changing the primitive's general storage or type compatibility.
fn adapter_owner(
    module: &BodyIrModule,
    builder: &Builder,
    slot: &CanonicalSymbolId,
    receiver: &PlanType,
) -> Result<Option<usize>, String> {
    let mut selected: Option<usize> = None;
    for (owner, declaration) in module.nominal_declarations.iter().enumerate() {
        for adoption in &declaration.trait_adoptions {
            let Some(adapter) = &adoption.primitive_adapter else {
                continue;
            };
            if !adapter.is_well_formed_for(declaration, adoption) {
                return Err("invalid Body IR primitive source adapter".into());
            }
            let matches_slot = adoption.methods.as_ref().is_some_and(|methods| methods.contains(slot));
            if !matches_slot {
                continue;
            }
            let expected = native_source_type(
                builder.constructors(),
                builder.nominals(),
                builder.enums(),
                &builder.unions,
                &adapter.receiver_type,
                &builder.intrinsics,
                "",
            )?;
            if expected != *receiver {
                continue;
            }
            if selected.is_some_and(|previous| previous != owner) {
                return Err("unsupported Body IR conflicting primitive source adapters".into());
            }
            selected = Some(owner);
        }
    }
    Ok(selected)
}

/// Project a newly allocated plan temporary through the existing local offset, with an explicit index bound.
fn temporary_source_place(builder: &Builder, local: i64) -> Result<SourcePlace, String> {
    let index = local - builder.local_offset - 1;
    let Ok(slot) = u32::try_from(index) else {
        return Err("unsupported Body IR primitive adapter temporary index".into());
    };
    Ok(SourcePlace::from_local(LocalId(slot)))
}
