//! The native Race statement: two owned futures raced through the SDK's source-order race with planned winner
//! callbacks. These are the Race steps of the Incan `statements` module.

use incan_mir_plan_native::{CalleeKind, Function, Operand, Parameter, Place, PlanType, SourceSpan};
use incan_semantics_core::body_ir::{
    Block, BodyIrModule, Place as SourcePlace, RaceArm, Rvalue, StatementKind as SourceStatementKind,
};

use crate::async_::{await_future, construct_boxed_future};
use crate::checked_values::scalar_place;
use crate::closure_lifting::{adopt_lifted, bind_closure_input, lifted_frame, synthesize};
use crate::frame::{Builder, place_type, read_place, temporary};
use crate::generator_expressions::step_pointer;
use crate::numerics::numeric_carrier;
use crate::statements::{finish_return, lower_statement, return_operand, source_future_place};

/// Run two owned native futures through the SDK's source-order race with planned winner callbacks.
pub fn lower_native_race(
    module: &BodyIrModule,
    builder: &mut Builder,
    destination: &Option<SourcePlace>,
    arms: &[RaceArm],
    span: &SourceSpan,
) -> Result<(), String> {
    let target = race_destination(builder, destination, span)?;
    let ty = place_type(&builder.locals, &target);
    if !numeric_carrier(&ty) && ty != PlanType::Bool && ty != PlanType::Unit {
        return Err("unsupported Body IR Race owned result".into());
    }
    if arms.len() != 2 {
        return Err("unsupported Body IR native Race arm count".into());
    }
    let mut futures: Vec<Place> = Vec::with_capacity(2);
    let mut inputs: Vec<PlanType> = Vec::with_capacity(2);
    let mut callbacks: Vec<Operand> = Vec::with_capacity(2);
    for arm in arms {
        futures.push(source_future_place(builder, &arm.awaitable, span)?);
        let binding = arm.binding.index() as i64 + 1 + builder.local_offset;
        let input = builder.locals[binding as usize].ty.clone();
        inputs.push(input.clone());
        callbacks.push(race_callback(module, builder, arm, input, &ty, span)?);
    }
    let types = vec![
        inputs[0].clone(),
        inputs[1].clone(),
        ty.clone(),
        place_type(&builder.locals, &futures[0]),
        place_type(&builder.locals, &futures[1]),
    ];
    let parameters = vec![
        types[3].clone(),
        PlanType::FunctionPointer(vec![inputs[0].clone(), ty.clone()]),
        types[4].clone(),
        PlanType::FunctionPointer(vec![inputs[1].clone(), ty.clone()]),
    ];
    let future = temporary(builder, PlanType::Future(vec![ty.clone()]), span);
    let mut callbacks = callbacks.into_iter();
    let (Some(first), Some(second)) = (callbacks.next(), callbacks.next()) else {
        return Err("unsupported Body IR native Race arm count".into());
    };
    let operands = vec![
        read_place(futures[0].clone(), true, span),
        first,
        read_place(futures[1].clone(), true, span),
        second,
    ];
    construct_boxed_future(
        builder,
        CalleeKind::Instantiated("incan_native_runtime::race_pair".into(), types),
        parameters,
        ty,
        operands,
        future.clone(),
        span,
    )?;
    await_future(builder, future, target, span)
}

/// Lift an arm's explicit Body IR block; enclosing lexical values stay refused until captures are retained.
fn race_callback(
    module: &BodyIrModule,
    builder: &mut Builder,
    arm: &RaceArm,
    input: PlanType,
    result: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let name = format!("__incan_race_callback_{}_{}", builder.name, builder.synthesized.len());
    let mut lifted = lifted_frame(builder, &name, std::slice::from_ref(&input), result.clone(), span);
    let binding = arm.binding.index() as i64 + 2;
    let root = arm.body.scope.0 as i64;
    let mut forbidden = Vec::new();
    for (local, scope) in &lifted.scoped_locals {
        if *local != binding && !scope_within(&lifted, *scope, root) {
            forbidden.push(*local);
        }
    }
    lifted.forbidden_reads.extend(forbidden);
    bind_closure_input(&mut lifted, binding, 1, &input, &input, span)?;
    lower_race_callback(module, &mut lifted, &arm.body)?;
    let value = return_operand(&mut lifted, &arm.result, span)?;
    finish_return(&mut lifted, value, span)?;
    for nested in std::mem::take(&mut lifted.synthesized) {
        synthesize(builder, nested);
    }
    adopt_lifted(builder, &lifted);
    let function = Function::new(
        name.clone(),
        vec![Parameter {
            name: "value".into(),
            ty: input.clone(),
            span: span.clone(),
        }],
        result.clone(),
        lifted.locals,
        lifted.blocks,
        span.clone(),
    );
    synthesize(builder, function);
    Ok(step_pointer(builder, &name, vec![input, result.clone()], span))
}

/// Use retained lexical ancestry to distinguish callback locals from unretained enclosing captures.
fn scope_within(builder: &Builder, scope: i64, root: i64) -> bool {
    let mut current = scope;
    for _ in 0..=builder.scope_parents.len() {
        if current == root {
            return true;
        }
        let mut parent = -1;
        for (child, ancestor) in &builder.scope_parents {
            if *child == current {
                parent = *ancestor;
            }
        }
        if parent < 0 {
            return false;
        }
        current = parent;
    }
    false
}

/// Require the frontend's shared race result slot before building any winner branch.
fn race_destination(
    builder: &mut Builder,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<Place, String> {
    match destination {
        Some(place) => scalar_place(builder, place, span),
        None => Err("unsupported Body IR Race without destination".into()),
    }
}

/// Lower straight-line winner callbacks; unretained control transfers and suspension remain refused.
fn lower_race_callback(module: &BodyIrModule, builder: &mut Builder, body: &Block) -> Result<(), String> {
    for statement in &body.stmts {
        match &statement.kind {
            SourceStatementKind::If { .. } | SourceStatementKind::Loop { .. } => {
                return Err("unsupported Body IR Race callback control flow".into());
            }
            SourceStatementKind::Return { .. } => return Err("unsupported Body IR Race callback Return".into()),
            SourceStatementKind::Break { .. } => return Err("unsupported Body IR Race callback Break".into()),
            SourceStatementKind::Continue => return Err("unsupported Body IR Race callback Continue".into()),
            SourceStatementKind::Race { .. } => return Err("unsupported Body IR nested Race callback".into()),
            SourceStatementKind::TryPropagate { .. } => {
                return Err("unsupported Body IR Race callback TryPropagate".into());
            }
            SourceStatementKind::Await { .. } => return Err("unsupported Body IR Race callback Await".into()),
            SourceStatementKind::Assign { rvalue, .. } => {
                if let Rvalue::Match { .. } = rvalue {
                    return Err("unsupported Body IR Race callback Match".into());
                }
                lower_statement(module, builder, statement)?;
            }
            _ => lower_statement(module, builder, statement)?,
        }
    }
    Ok(())
}
