//! Forward runtime-owned arguments into their published Incan frame's borrowing contract.

use incan_mir_plan_native::{
    BasicBlock, Callee, CalleeKind, ExternalFunction, Function, Local, Operand, Parameter, Place, PlanType, RvalueKind,
    SourceSpan, Statement, TerminatorKind, Unwind, assign as plan_assign, block as plan_block, place as plan_place,
};
use incan_semantics_core::body_ir::BodyIrModule;

use crate::frame::read_place;
use crate::layout_types::{native_alias_boundary, shared_receiver_type};

/// Forward a runtime call: 1. Retain its inputs. 2. Adapt declared storage. 3. Clean up borrowed owners.
///
/// Runtime operations describe their owned native inputs. A published Incan value parameter can instead receive a
/// shared view of that owner. The declaration's frame supplies every target type; this forwarder adds no body
/// semantics and never infers a signature from an operand.
pub fn runtime_frame_adapter(
    module: &BodyIrModule,
    external: &ExternalFunction,
    parameters: &[PlanType],
    result: &PlanType,
    target: &str,
    callee: &CalleeKind,
) -> Result<Option<Function>, String> {
    // 1. A direct ABI needs no adapter; an arity mismatch grants no forwarding contract.
    if parameters == external.parameters.as_slice() && *result == external.return_type {
        return Ok(None);
    }
    if parameters.len() != external.parameters.len() {
        return Err(format!("invalid governed runtime frame arity for {}", external.path));
    }
    // 2. Owned input slots stay alive until every declared shared view is no longer in use.
    let span = &external.span;
    let nominals = &module.nominal_declarations;
    let mut locals = vec![Local {
        ty: external.return_type.clone(),
        span: span.clone(),
    }];
    let mut inputs = Vec::with_capacity(parameters.len());
    for (index, ty) in external.parameters.iter().enumerate() {
        inputs.push(Parameter {
            name: format!("__incan_argument_{index}"),
            ty: ty.clone(),
            span: span.clone(),
        });
        locals.push(Local {
            ty: ty.clone(),
            span: span.clone(),
        });
    }
    // 2. Adapt only declared borrowing and exact native aliases.
    let mut statements: Vec<Statement> = Vec::new();
    let mut operands: Vec<Operand> = Vec::new();
    let mut owners: Vec<Place> = Vec::new();
    for (index, expected) in parameters.iter().enumerate() {
        let source = plan_place(index as i64 + 1, span);
        let actual = &external.parameters[index];
        if actual == expected {
            operands.push(read_place(source, true, span));
            continue;
        }
        if native_alias_boundary(nominals, actual, expected) {
            let alias = plan_place(locals.len() as i64, span);
            locals.push(Local {
                ty: expected.clone(),
                span: span.clone(),
            });
            let aliased = RvalueKind::NativeAlias(read_place(source, true, span), actual.clone(), expected.clone());
            statements.push(plan_assign(alias.clone(), aliased, span));
            operands.push(read_place(alias, true, span));
            continue;
        }
        let borrowed = shared_receiver_type(actual.clone())?;
        if borrowed != *expected && !native_alias_boundary(nominals, &borrowed, expected) {
            return Err(format!(
                "unsupported governed runtime frame argument {actual:?} to {expected:?} for {}",
                external.path
            ));
        }
        let reference = plan_place(locals.len() as i64, span);
        locals.push(Local {
            ty: borrowed.clone(),
            span: span.clone(),
        });
        statements.push(plan_assign(reference.clone(), RvalueKind::Borrow(source.clone()), span));
        let mut argument = reference.clone();
        if borrowed != *expected {
            argument = plan_place(locals.len() as i64, span);
            locals.push(Local {
                ty: expected.clone(),
                span: span.clone(),
            });
            let aliased = RvalueKind::NativeAlias(read_place(reference, false, span), borrowed, expected.clone());
            statements.push(plan_assign(argument.clone(), aliased, span));
        }
        operands.push(read_place(argument, false, span));
        owners.push(source);
    }
    // 3. Both completion paths release only owners whose views were lent to the target frame.
    let mut blocks: Vec<BasicBlock> = vec![plan_block(Vec::new(), TerminatorKind::Unreachable, span)];
    let mut output = plan_place(0, span);
    let mut normal = owner_cleanup(&mut blocks, &owners, false, span);
    if *result != external.return_type {
        if !native_alias_boundary(nominals, result, &external.return_type) {
            return Err(format!(
                "unsupported governed runtime result alias for {}",
                external.path
            ));
        }
        output = plan_place(locals.len() as i64, span);
        locals.push(Local {
            ty: result.clone(),
            span: span.clone(),
        });
        let cleanup = normal;
        normal = blocks.len() as i64;
        let aliased = RvalueKind::NativeAlias(
            read_place(output.clone(), true, span),
            result.clone(),
            external.return_type.clone(),
        );
        blocks.push(plan_block(
            vec![plan_assign(plan_place(0, span), aliased, span)],
            TerminatorKind::Goto(cleanup),
            span,
        ));
    }
    let cleanup = owner_cleanup(&mut blocks, &owners, true, span);
    let call = TerminatorKind::Call(
        Callee {
            kind: callee.clone(),
            span: span.clone(),
        },
        operands,
        output,
        normal,
        Unwind::Cleanup(cleanup),
    );
    blocks[0] = plan_block(statements, call, span);
    Ok(Some(Function::new(
        format!("{target}__runtime_frame"),
        inputs,
        external.return_type.clone(),
        locals,
        blocks,
        span.clone(),
    )))
}

/// Release only the owners borrowed by the forwarder; transferred arguments belong to the target frame.
fn owner_cleanup(blocks: &mut Vec<BasicBlock>, owners: &[Place], unwinding: bool, span: &SourceSpan) -> i64 {
    let first = blocks.len() as i64;
    for owner in owners.iter().rev() {
        let unwind = if unwinding { Unwind::Terminate } else { Unwind::Continue };
        let next = blocks.len() as i64 + 1;
        let mut block = plan_block(Vec::new(), TerminatorKind::Drop(owner.clone(), next, unwind), span);
        block.cleanup = unwinding;
        blocks.push(block);
    }
    let finished = if unwinding {
        TerminatorKind::UnwindResume
    } else {
        TerminatorKind::Return
    };
    let mut block = plan_block(Vec::new(), finished, span);
    block.cleanup = unwinding;
    blocks.push(block);
    first
}
