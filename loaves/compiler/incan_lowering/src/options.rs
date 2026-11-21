//! Concrete standard Option and Result layouts shared with native enum operations.

use incan_lang::lang::types::collections::{CollectionTypeId, from_str as collection_type};
use incan_mir_plan_native::{
    CalleeKind, Carrier, EnumDeclaration, EnumVariant, ExternalFunction, Operand as PlanOperand, Place, PlanType,
    Projection, RvalueKind, SourceSpan, TerminatorKind, Unwind, assign as plan_assign, source_span,
};
use incan_semantics_core::body_ir::{
    Block, Body, BodyIrModule, CallableTarget, Callee, MatchArm, NamedCallableTarget, Operand, Rvalue, StatementKind,
    StaticInitializer,
};
use incan_semantics_core::{IncanType, SemanticSourceTargetKind, generic};

use crate::compiler_record_adapters::{authored_plan_checked_type, semantic_key_matches_native};
use crate::frame::{
    Builder, cleanup_exit, drop_owned, forget_owned, place_type, read_place, temporary, terminate_call,
    track_rvalue_moves,
};
use crate::layout_types::native_source_type;
use crate::lists::taking_iterator_locals;
use crate::native_type_identity::{ConstructorIndex, native_owner};
use crate::nominal_frames::{closed_arguments, closed_type, declaration_storage};
use crate::numerics::numeric_carrier;
use crate::text_plan::clone_text;
use crate::tuples::{is_tuple_type, tuple_fields, tuple_has_owners};
use crate::type_parameters::substitute;
use crate::unions::lower_union_declaration;

/// Retain concrete carriers after any layouts whose indices an earlier lowering already published.
pub fn intrinsic_declarations(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    file: &str,
    retained: Vec<EnumDeclaration>,
) -> Result<Vec<EnumDeclaration>, String> {
    let mut declarations = retained;
    for ty in declaration_storage(module) {
        if closed_type(&ty) {
            retain_carrier(constructors, module, &mut declarations, &ty, file)?;
        }
    }
    for delegation in &module.loaf_callables {
        if !delegation.type_parameters.is_empty() || !closed_arguments(&delegation.native_type_arguments) {
            continue;
        }
        for parameter in &delegation.parameters {
            if closed_type(parameter) {
                retain_carrier(constructors, module, &mut declarations, parameter, file)?;
            }
        }
        // A checked native instantiation may still contain its enclosing source function's binders. Its closed
        // function frame schedules those carriers; the declaration template does not authorize an open layout.
        if closed_type(&delegation.return_type) {
            retain_carrier(constructors, module, &mut declarations, &delegation.return_type, file)?;
        }
    }
    for declaration in &module.nominal_declarations {
        for ty in &declaration.field_types {
            // A Rust layout keeps a field without a carrier opaque; that field grants no read, write or construction.
            if native_owner(declaration) {
                let _ = retain_carrier(constructors, module, &mut declarations, ty, file);
            } else {
                retain_carrier(constructors, module, &mut declarations, ty, file)?;
            }
        }
    }
    for declaration in &module.enum_declarations {
        for variant in &declaration.variants {
            for ty in &variant.fields {
                retain_carrier(constructors, module, &mut declarations, ty, file)?;
            }
        }
    }
    for body in &module.bodies {
        if !body.type_parameters.is_empty() {
            continue;
        }
        retain_body_carriers(constructors, module, &mut declarations, body, file)?;
    }
    for declaration in &module.static_declarations {
        retain_carrier(constructors, module, &mut declarations, &declaration.ty, file)?;
        if let StaticInitializer::Computed(body) = &declaration.initial {
            retain_body_carriers(constructors, module, &mut declarations, body, file)?;
        }
    }
    Ok(declarations)
}

/// Append closed frame and poll carriers without renumbering layouts; refusal names its declaring frame position.
pub fn retain_body_carriers(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    declarations: &mut Vec<EnumDeclaration>,
    body: &Body,
    file: &str,
) -> Result<(), String> {
    retain_carrier(constructors, module, declarations, &body.return_type, file)
        .map_err(|reason| format!("{reason} in {} return carrier", body.name))?;
    for (index, local) in body.locals.iter().enumerate() {
        retain_carrier(constructors, module, declarations, &local.ty, file)
            .map_err(|reason| format!("{reason} in {} local {index} carrier", body.name))?;
    }
    let taking = taking_iterator_locals(&body.block);
    retain_iteration_poll_carriers(constructors, module, declarations, body, &body.block, &taking, file)
        .map_err(|reason| format!("{reason} in {} iteration carriers", body.name))
}

/// Retain Iterator::next's Option output from a checked consuming loop's concrete item type before frame lowering.
fn retain_iteration_poll_carriers(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    declarations: &mut Vec<EnumDeclaration>,
    body: &Body,
    block: &Block,
    taking: &[i64],
    file: &str,
) -> Result<(), String> {
    for statement in &block.stmts {
        retain_iteration_poll_statement(constructors, module, declarations, body, &statement.kind, taking, file)?;
    }
    Ok(())
}

fn retain_iteration_poll_statement(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    declarations: &mut Vec<EnumDeclaration>,
    body: &Body,
    kind: &StatementKind,
    taking: &[i64],
    file: &str,
) -> Result<(), String> {
    match kind {
        StatementKind::Call {
            callee: Callee::Function(CallableTarget::Named(target)),
            ..
        } => {
            retain_call_carriers(constructors, module, declarations, target, file)?;
        }
        StatementKind::IterNext {
            destination, iterator, ..
        } => {
            if let Operand::Place(read) = iterator
                && let Some(local) = read.place.local_id()
            {
                let index = local.index() as i64 + 1;
                if taking.contains(&index) {
                    let Some(item) = destination.local_id() else {
                        return Err("unsupported Body IR consuming iteration global item".into());
                    };
                    let ty = body.locals[item.index()].ty.clone();
                    let output = generic("Option".into(), vec![ty]);
                    retain_carrier(constructors, module, declarations, &output, file)?;
                }
            }
        }
        StatementKind::Assign {
            rvalue: Rvalue::Match { arms, .. },
            ..
        } => {
            for arm in arms {
                retain_poll_arm(constructors, module, declarations, body, arm, taking, file)?;
            }
        }
        StatementKind::Race { arms, .. } => {
            for arm in arms {
                retain_iteration_poll_carriers(constructors, module, declarations, body, &arm.body, taking, file)?;
            }
        }
        StatementKind::Loop { body: child } | StatementKind::UnsafeRegion { body: child, .. } => {
            retain_iteration_poll_carriers(constructors, module, declarations, body, child, taking, file)?;
        }
        StatementKind::If {
            then_block, else_block, ..
        } => {
            retain_iteration_poll_carriers(constructors, module, declarations, body, then_block, taking, file)?;
            if let Some(child) = else_block {
                retain_iteration_poll_carriers(constructors, module, declarations, body, child, taking, file)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn retain_poll_arm(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    declarations: &mut Vec<EnumDeclaration>,
    body: &Body,
    arm: &MatchArm,
    taking: &[i64],
    file: &str,
) -> Result<(), String> {
    for statement in arm.guard_stmts.iter().chain(&arm.body_stmts) {
        retain_iteration_poll_statement(constructors, module, declarations, body, &statement.kind, taking, file)?;
    }
    Ok(())
}

/// Retain closed source and published free-function signatures, including carriers absent from locals.
fn retain_call_carriers(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    declarations: &mut Vec<EnumDeclaration>,
    target: &NamedCallableTarget,
    file: &str,
) -> Result<(), String> {
    match &target.canonical {
        Some(identity) if identity.kind != SemanticSourceTargetKind::Function => return Ok(()),
        None => return Ok(()),
        _ => {}
    }
    if !closed_arguments(&target.type_args) {
        return Ok(());
    }
    for candidate in &module.bodies {
        if target.canonical != candidate.canonical || candidate.type_parameters.len() != target.type_args.len() {
            continue;
        }
        let parameters: Vec<IncanType> = candidate.params.iter().map(|parameter| parameter.ty.clone()).collect();
        retain_closed_signature(
            constructors,
            module,
            declarations,
            &parameters,
            &candidate.return_type,
            &candidate.type_parameters,
            &target.type_args,
            file,
        )?;
    }
    for delegation in &module.loaf_callables {
        if target.canonical.as_ref() != Some(&delegation.canonical) {
            continue;
        }
        if target.type_args.len() != delegation.type_parameters.len() {
            return Err("the standard library carrier type argument count is unsupported".into());
        }
        retain_closed_signature(
            constructors,
            module,
            declarations,
            &delegation.parameters,
            &delegation.return_type,
            &delegation.type_parameters,
            &target.type_args,
            file,
        )?;
    }
    Ok(())
}

/// Close a retained callable's signature from checked type arguments before any operand needs its carrier.
#[allow(clippy::too_many_arguments)]
fn retain_closed_signature(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    declarations: &mut Vec<EnumDeclaration>,
    parameters: &[IncanType],
    result: &IncanType,
    names: &[String],
    arguments: &[IncanType],
    file: &str,
) -> Result<(), String> {
    for parameter in parameters {
        let instantiated = substitute(parameter, names, arguments)?;
        retain_carrier(constructors, module, declarations, &instantiated, file)?;
    }
    let closed_result = substitute(result, names, arguments)?;
    retain_carrier(constructors, module, declarations, &closed_result, file)
}

/// Retain one concrete standard layout once; nested carriers precede their containing carrier.
pub fn retain_carrier(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    declarations: &mut Vec<EnumDeclaration>,
    ty: &IncanType,
    file: &str,
) -> Result<(), String> {
    match ty {
        IncanType::Tuple(elements) => {
            // A carrier held only as a tuple element still needs its own layout.
            for element in elements {
                retain_carrier(constructors, module, declarations, element, file)?;
            }
        }
        IncanType::Generic { base, args } => {
            for argument in args {
                retain_carrier(constructors, module, declarations, argument, file)?;
            }
            let kind = collection_type(base);
            let is_option = kind == Some(CollectionTypeId::Option);
            let is_result = kind == Some(CollectionTypeId::Result);
            if !is_option && !is_result && !ty.is_union() {
                return Ok(());
            }
            for retained in declarations.iter() {
                if semantic_key_matches_native(&retained.semantic_key, ty)? {
                    return Ok(());
                }
            }
            if ty.is_union() {
                let declaration = lower_union_declaration(constructors, module, ty, file, declarations)?;
                declarations.push(declaration);
                return Ok(());
            }
            if (is_option && args.len() != 1) || (is_result && args.len() != 2) {
                return Err(format!("unsupported Body IR type {ty}"));
            }
            let mut fields = Vec::with_capacity(args.len());
            for (index, argument) in args.iter().enumerate() {
                let field = native_source_type(
                    constructors,
                    &module.nominal_declarations,
                    &module.enum_declarations,
                    &[],
                    argument,
                    declarations,
                    "",
                )
                .map_err(|reason| format!("{reason} in {base} payload {index} of {ty}"))?;
                fields.push(field);
            }
            let (variants, carrier) = if is_result {
                (
                    vec![
                        EnumVariant {
                            name: "Ok".into(),
                            fields: vec![fields[0].clone()],
                        },
                        EnumVariant {
                            name: "Err".into(),
                            fields: vec![fields[1].clone()],
                        },
                    ],
                    Carrier::Result,
                )
            } else {
                (
                    vec![
                        EnumVariant {
                            name: "None".into(),
                            fields: Vec::new(),
                        },
                        EnumVariant {
                            name: "Some".into(),
                            fields: vec![fields[0].clone()],
                        },
                    ],
                    Carrier::Option,
                )
            };
            let semantic_key =
                authored_plan_checked_type(ty, file, &constructors.source_files, &constructors.package_versions)?;
            let mut declaration = EnumDeclaration::new(
                format!("__IncanCarrier{}", declarations.len()),
                false,
                variants,
                vec!["Clone".into()],
                source_span(file, 1, 1),
            );
            declaration.carrier = carrier;
            declaration.semantic_key = Some(semantic_key);
            declaration.source_type = ty.to_string();
            declarations.push(declaration);
        }
        IncanType::Function { params, return_type } => {
            for parameter in params {
                retain_carrier(constructors, module, declarations, &parameter.ty, file)?;
            }
            retain_carrier(constructors, module, declarations, return_type, file)?;
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => {
            retain_carrier(constructors, module, declarations, inner, file)?;
        }
        _ => {}
    }
    Ok(())
}

/// Evaluate an owned replacement before dropping its old field, keeping the enclosing owner initialized.
pub fn replace_owned_destination(builder: &mut Builder, target: Place, operation: RvalueKind, span: &SourceSpan) {
    if target.projection == Projection::Whole {
        track_rvalue_moves(builder, &operation);
        drop_owned(builder, target.local, span);
        let local = target.local;
        builder.emit(plan_assign(target, operation, span));
        builder.owned.push(local);
        return;
    }
    let replacement = temporary(builder, place_type(&builder.locals, &target), span);
    track_rvalue_moves(builder, &operation);
    builder.emit(plan_assign(replacement.clone(), operation, span));
    builder.owned.push(replacement.local);
    let continuation = builder.new_block(span);
    let cleanup = cleanup_exit(builder, span);
    builder.finish(
        TerminatorKind::Drop(target.clone(), continuation, Unwind::Cleanup(cleanup)),
        span,
    );
    builder.select(continuation);
    forget_owned(builder, replacement.local);
    builder.emit(plan_assign(
        target,
        RvalueKind::Use(read_place(replacement, true, span)),
        span,
    ));
}

/// Copy primitive, static frozen, function-pointer and immutable scalar-reference payloads, or clone an owned active
/// payload.
pub fn carrier_payload_value(builder: &mut Builder, place: Place, span: &SourceSpan) -> Result<PlanOperand, String> {
    let ty = place_type(&builder.locals, &place);
    if numeric_carrier(&ty) || matches!(ty, PlanType::Bool | PlanType::Unit) {
        return Ok(read_place(place, false, span));
    }
    match &ty {
        PlanType::FrozenStr
        | PlanType::FrozenBytes
        | PlanType::FunctionPointer(_)
        | PlanType::ScalarRef(_)
        | PlanType::StringRef
        | PlanType::ListRef(..)
        | PlanType::SetRef(_)
        | PlanType::DictRef(..)
        | PlanType::TupleRef(_)
        | PlanType::ModelRef(..)
        | PlanType::EnumRef(..)
        | PlanType::ValidationErrorRef => return Ok(read_place(place, false, span)),
        PlanType::Tuple(_) => return clone_tuple_operand(builder, place, span),
        _ => {}
    }
    let reference_type = match &ty {
        PlanType::String => PlanType::StringRef,
        PlanType::ValidationError => PlanType::ValidationErrorRef,
        PlanType::List(leaf, depth) => PlanType::ListRef(leaf.clone(), *depth),
        PlanType::Set(leaf) => PlanType::SetRef(leaf.clone()),
        PlanType::Dict(key, value) => PlanType::DictRef(key.clone(), value.clone()),
        PlanType::Enum(owner, name) => PlanType::EnumRef(*owner, name.clone()),
        PlanType::Model(owner, name) => PlanType::ModelRef(*owner, name.clone()),
        PlanType::FrozenCollection(name, elements) => PlanType::FrozenCollectionRef(name.clone(), elements.clone()),
        _ => {
            return Err(format!(
                "unsupported Body IR intrinsic payload clone type {ty:?} in {} at {}",
                builder.name, span.line
            ));
        }
    };
    let output = borrowed_native_value(
        builder,
        "core::clone::Clone::clone",
        place,
        ty.clone(),
        reference_type,
        ty,
        span,
    );
    Ok(read_place(output, true, span))
}

/// Snapshot tuple elements without partially moving an owned tuple, cloning each owned element in place.
pub fn clone_tuple_operand(builder: &mut Builder, value: Place, span: &SourceSpan) -> Result<PlanOperand, String> {
    let ty = place_type(&builder.locals, &value);
    let mut operands = Vec::new();
    for field in tuple_fields(&value, &ty, span)? {
        let field_type = place_type(&builder.locals, &field);
        if field_type == PlanType::String {
            operands.push(read_place(clone_text(builder, field, span)?, true, span));
        } else if is_tuple_type(&field_type) {
            operands.push(clone_tuple_operand(builder, field, span)?);
        } else {
            // Scalars are copied; an owned list or enum carrier is cloned through its real Clone implementation.
            operands.push(carrier_payload_value(builder, field, span)?);
        }
    }
    let result = temporary(builder, ty.clone(), span);
    let operation = RvalueKind::Tuple(operands);
    track_rvalue_moves(builder, &operation);
    builder.emit(plan_assign(result.clone(), operation, span));
    let owned = tuple_has_owners(&ty);
    if owned {
        builder.owned.push(result.local);
    }
    Ok(read_place(result, true, span))
}

/// Instantiate one native callee against a checked carrier or payload type and its shared reference.
pub fn borrowed_native_value(
    builder: &mut Builder,
    path: &str,
    source: Place,
    ty: PlanType,
    reference_type: PlanType,
    result_type: PlanType,
    span: &SourceSpan,
) -> Place {
    let reference = temporary(builder, reference_type.clone(), span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(source), span));
    let output = temporary(builder, result_type.clone(), span);
    builder.externals.push(ExternalFunction {
        path: path.to_string(),
        type_arguments: vec![ty.clone()],
        parameters: vec![reference_type],
        return_type: result_type,
        span: span.clone(),
    });
    terminate_call(
        builder,
        CalleeKind::Instantiated(path.to_string(), vec![ty]),
        vec![read_place(reference, false, span)],
        output.clone(),
        span,
    );
    builder.owned.push(output.local);
    output
}
