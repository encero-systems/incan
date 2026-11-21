//! Derived `Default` construction for `Type.default()` and `T.default()` under a `Default` bound.
//!
//! Legacy implements a derived Default by building the value: each field takes its declared default, and a field that
//! declares none takes its type's own default. The direct route builds that same value at the call, so no native
//! struct derives Default and no field default is ever replaced by a zero value.

use incan_mir_plan_native::{Constant, Operand, OperandKind, PlanType, RvalueKind, SourceSpan, assign as plan_assign};
use incan_semantics_core::body_ir::{
    BodyIrModule, CallableParamDefault, NamedCallableTarget, Place as SourcePlace, nominal_carrier_key,
};
use incan_semantics_core::{IncanType, SemanticSourceTargetKind, SymbolOrigin};

use crate::builtins::list_call;
use crate::checked_values::scalar_place;
use crate::collections::{collection_call, is_collection};
use crate::defaults::evaluate_default;
use crate::frame::{Builder, drop_owned, forget_owned, literal, place_type, read_place, temporary, track_rvalue_moves};
use crate::layout_types::native_source_type;
use crate::lists::list_element;
use crate::numerics::{numeric_carrier, numeric_zero};
use crate::options::none_operand;
use crate::text_plan::owned_literal;
use crate::type_parameters::substitute;

/// Recognize the std `Default` trait's `default` in source and compiled SDK identities, never a same-name method.
pub fn is_derived_default(target: &NamedCallableTarget) -> bool {
    let Some(identity) = &target.canonical else {
        return false;
    };
    if identity.kind != SemanticSourceTargetKind::Method || identity.declaration_name != "default" {
        return false;
    }
    match &identity.origin {
        SymbolOrigin::Module(path) => {
            path.len() == 3 && path[0] == "std" && path[1] == "derives" && path[2] == "copying"
        }
        SymbolOrigin::Package { library, module_path } => {
            library == "incan_stdlib_primitives"
                && module_path.len() == 2
                && module_path[0] == "derives"
                && module_path[1] == "copying"
        }
        _ => false,
    }
}

/// Build the receiver model's derived default into the call's destination, replacing any value held there.
pub fn lower_derived_default(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &NamedCallableTarget,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if !target.type_args.is_empty() {
        return Err("unsupported Body IR derived Default with type arguments".into());
    }
    let ty = receiver_type(builder, target, destination, span)?;
    if let Some(place) = destination {
        let output = scalar_place(builder, place, span)?;
        if place_type(&builder.locals, &output) != ty {
            return Err("unsupported Body IR derived Default receiver and result mismatch".into());
        }
    }
    let value = default_value(module, builder, &ty, span)?;
    if let Some(place) = destination {
        let output = scalar_place(builder, place, span)?;
        drop_owned(builder, output.local, span);
        if let OperandKind::Move(result) = &value.kind {
            forget_owned(builder, result.local);
        }
        let local = output.local;
        builder.emit(plan_assign(output, RvalueKind::Use(value), span));
        builder.owned.push(local);
    }
    Ok(())
}

/// Close a checked receiver; an associated generic owner takes arguments only from its exact canonical result layout.
fn receiver_type(
    builder: &mut Builder,
    target: &NamedCallableTarget,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<PlanType, String> {
    let Some(ty) = &target.receiver_type else {
        return Err("unsupported Body IR derived Default without a receiver type".into());
    };
    let closed = substitute(ty, &builder.type_parameters, &builder.type_arguments)?;
    if let IncanType::Named(name) = &closed
        && let Some(place) = destination
    {
        let output = scalar_place(builder, place, span)?;
        let result = place_type(&builder.locals, &output);
        if let PlanType::Model(owner, _) = &result {
            let declaration = &builder.nominals()[*owner as usize];
            if declaration.type_parameter_count != 0
                && nominal_carrier_key(&declaration.canonical).as_deref() == Some(name.to_string().as_str())
            {
                return Ok(result);
            }
        }
    }
    native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &closed,
        &builder.intrinsics,
        "",
    )
}

/// Build a model that derives Default from its fields; the result is a new owner the caller moves.
fn default_value(
    module: &BodyIrModule,
    builder: &mut Builder,
    ty: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let PlanType::Model(owner, _) = ty else {
        return Err("unsupported Body IR derived Default receiver".into());
    };
    if !builder.nominals()[*owner as usize]
        .derives
        .iter()
        .any(|derive| derive == "Default")
    {
        return Err("unsupported Body IR Default of a model that does not derive it".into());
    }
    let operands = default_fields(module, builder, *owner, span)?;
    let result = temporary(builder, ty.clone(), span);
    let operation = RvalueKind::Model(*owner, operands);
    track_rvalue_moves(builder, &operation);
    builder.emit(plan_assign(result.clone(), operation, span));
    builder.owned.push(result.local);
    Ok(read_place(result, true, span))
}

/// Evaluate each field in declaration order: its declared default, or its type's default when it declares none.
///
/// A declared default evaluates through `defaults::evaluate_default`.
fn default_fields(
    module: &BodyIrModule,
    builder: &mut Builder,
    owner: i64,
    span: &SourceSpan,
) -> Result<Vec<Operand>, String> {
    let declaration = builder.nominals()[owner as usize].clone();
    let mut operands = Vec::with_capacity(declaration.fields.len());
    for (slot, field_type) in declaration
        .field_types
        .iter()
        .enumerate()
        .take(declaration.fields.len())
    {
        let ty = native_source_type(
            builder.constructors(),
            builder.nominals(),
            builder.enums(),
            &builder.unions,
            field_type,
            &builder.intrinsics,
            "",
        )?;
        if let Some(body) = &declaration.field_default_body
            && let CallableParamDefault::Source(_) = body.params[slot].default
        {
            operands.push(evaluate_default(module, builder, body, slot, &ty, span, true)?);
            continue;
        }
        operands.push(type_default(module, builder, &ty, span)?);
    }
    Ok(operands)
}

/// The value a field type's own Default gives: zero, empty text or collection, None, or a nested derived default.
fn type_default(
    module: &BodyIrModule,
    builder: &mut Builder,
    ty: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if numeric_carrier(ty) {
        return Ok(literal(numeric_zero(ty)?, span));
    }
    match ty {
        PlanType::Bool => return Ok(literal(Constant::Bool(false), span)),
        PlanType::String => return Ok(read_place(owned_literal(builder, "", span), true, span)),
        PlanType::List(leaf, depth) => {
            let output = temporary(builder, ty.clone(), span);
            let element = list_element(leaf, *depth);
            list_call(
                builder,
                "empty_list",
                element,
                Vec::new(),
                ty.clone(),
                Vec::new(),
                output.clone(),
                span,
            );
            return Ok(read_place(output, true, span));
        }
        PlanType::Model(..) => return default_value(module, builder, ty, span),
        PlanType::Enum(..) => return none_operand(builder, ty, span),
        _ => {}
    }
    if is_collection(ty) {
        let output = temporary(builder, ty.clone(), span);
        collection_call(
            builder,
            ty,
            "empty",
            Vec::new(),
            ty.clone(),
            Vec::new(),
            output.clone(),
            span,
        )?;
        return Ok(read_place(output, true, span));
    }
    Err("unsupported Body IR derived Default field type".into())
}
