//! SDK callable storage for locally proven immutable primitive-capture environments.

use incan_mir_plan_native::{
    Callee, CalleeKind, Function, Local, Operand, OperandKind, Parameter, PlanType, SourceSpan, TerminatorKind, Unwind,
    block as plan_block, place as plan_place,
};

use crate::frame::{Builder, temporary, terminate_call};

/// Admit only immutable builtin values whose cloning has no user-defined behavior.
pub fn scalar_capture(ty: &PlanType) -> bool {
    matches!(
        ty,
        PlanType::String | PlanType::Int | PlanType::Float | PlanType::Bool | PlanType::Unit
    )
}

/// Keep every assignment safe at a lexical slot; one unproven incoming environment closes it permanently.
pub fn record_scalar_closure(builder: &mut Builder, local: i64, safe: bool) {
    if !safe {
        if !builder.unproven_closures.contains(&local) {
            builder.unproven_closures.push(local);
        }
        return;
    }
    if !builder.scalar_closures.contains(&local) {
        builder.scalar_closures.push(local);
    }
}

/// Transfer checked boxed inputs into a distinct cloneable SDK carrier, preserving ordinary argument slot order.
pub fn shared_sdk_operands(
    builder: &mut Builder,
    types: &[PlanType],
    values: Vec<Operand>,
    span: &SourceSpan,
) -> Result<Vec<Operand>, String> {
    if types.len() != values.len() {
        return Err("invalid Body IR shared callable argument slots".into());
    }
    let mut result = Vec::with_capacity(values.len());
    for (ty, value) in types.iter().zip(values) {
        let PlanType::SharedCallable(signature) = ty else {
            result.push(value);
            continue;
        };
        let name = format!("__incan_shared_callable_{}_{}", builder.name, builder.synthesized.len());
        builder.synthesized.push(shared_constructor(&name, signature, span));
        let target = temporary(builder, ty.clone(), span);
        terminate_call(builder, CalleeKind::Planned(name), vec![value], target.clone(), span);
        builder.owned.push(target.local);
        result.push(Operand {
            kind: OperandKind::Move(target),
            span: span.clone(),
        });
    }
    Ok(result)
}

/// Describe the one transfer the native bridge implements; its Rust-owned body wraps without executing the callback.
fn shared_constructor(name: &str, signature: &[PlanType], span: &SourceSpan) -> Function {
    let source = PlanType::Closure(signature.to_vec());
    let result = PlanType::SharedCallable(signature.to_vec());
    let value = Operand {
        kind: OperandKind::Move(plan_place(1, span)),
        span: span.clone(),
    };
    let call = TerminatorKind::Call(
        Callee {
            kind: CalleeKind::SharedCallableBody(signature.to_vec()),
            span: span.clone(),
        },
        vec![value],
        plan_place(0, span),
        1,
        Unwind::Continue,
    );
    let parameters = vec![Parameter {
        name: "__incan_callback".into(),
        ty: source.clone(),
        span: span.clone(),
    }];
    let locals = vec![
        Local {
            ty: result.clone(),
            span: span.clone(),
        },
        Local {
            ty: source,
            span: span.clone(),
        },
    ];
    let blocks = vec![
        plan_block(Vec::new(), call, span),
        plan_block(Vec::new(), TerminatorKind::Return, span),
    ];
    Function::new(name.to_owned(), parameters, result, locals, blocks, span.clone())
}
