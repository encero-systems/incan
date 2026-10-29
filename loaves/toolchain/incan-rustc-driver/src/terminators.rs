//! Native control-flow exits, including explicit cleanup edges and overflow assertions.

use crate::callees;
use crate::error::PlanError;
use crate::plan::{Terminator, TerminatorKind, Unwind};
use crate::spans::Sources;
use crate::values::{binary, index, operand, place};
use rustc_middle::mir;
use rustc_middle::ty::TyCtxt;
use rustc_span::Spanned;
use thin_vec::ThinVec;

/// Convert an admitted basic-block index to the pinned rustc index type.
fn block(value: i64) -> Result<mir::BasicBlock, PlanError> {
    Ok(mir::BasicBlock::from_usize(index(value)?))
}

/// Preserve each explicit unwind choice; never infer owned-value cleanup in the adapter.
pub(crate) fn unwind(value: &Unwind) -> Result<mir::UnwindAction, PlanError> {
    Ok(match value {
        Unwind::Continue => mir::UnwindAction::Continue,
        Unwind::Cleanup(target) => mir::UnwindAction::Cleanup(block(*target)?),
        Unwind::Terminate => mir::UnwindAction::Terminate(mir::UnwindTerminateReason::InCleanup),
    })
}

/// Lower one block exit, retaining source locations and removing the normal continuation of checked diverging calls.
pub fn terminator<'tcx>(
    tcx: TyCtxt<'tcx>,
    sources: &Sources<'_>,
    function: &crate::plan::Function,
    value: &Terminator,
) -> Result<mir::Terminator<'tcx>, PlanError> {
    let span = sources.span(&value.span)?;
    let kind = match &value.kind {
        TerminatorKind::Goto(target) => mir::TerminatorKind::Goto {
            target: block(*target)?,
        },
        TerminatorKind::SwitchBool(condition, false_target, true_target) => mir::TerminatorKind::SwitchInt {
            discr: operand(tcx, sources, condition)?,
            targets: mir::SwitchTargets::new([(0, block(*false_target)?)].into_iter(), block(*true_target)?),
        },
        TerminatorKind::Call(callee, arguments, destination, target, action) => mir::TerminatorKind::Call {
            func: call_operand(tcx, sources, callee)?,
            args: arguments
                .iter()
                .map(|value| {
                    Ok(Spanned {
                        node: operand(tcx, sources, value)?,
                        span: sources.span(&value.span)?,
                    })
                })
                .chain(generator_producer(tcx, sources, callee)?.into_iter().map(Ok))
                .collect::<Result<_, PlanError>>()?,
            destination: place(tcx, destination)?,
            target: if function
                .locals
                .get(index(destination.local)?)
                .is_some_and(|local| matches!(local.ty, crate::plan::PlanType::Never))
            {
                None
            } else {
                Some(block(*target)?)
            },
            unwind: unwind(action)?,
            call_source: mir::CallSource::Normal,
            fn_span: span,
        },
        TerminatorKind::Drop(value, target, action) => mir::TerminatorKind::Drop {
            place: place(tcx, value)?,
            target: block(*target)?,
            unwind: unwind(action)?,
            replace: false,
            drop: None,
        },
        TerminatorKind::AssertOverflow(condition, op, left, right, target, action) => mir::TerminatorKind::Assert {
            cond: operand(tcx, sources, condition)?,
            expected: false,
            msg: Box::new(mir::AssertKind::Overflow(
                binary(op, false),
                operand(tcx, sources, left)?,
                operand(tcx, sources, right)?,
            )),
            target: block(*target)?,
            unwind: unwind(action)?,
        },
        TerminatorKind::Return => mir::TerminatorKind::Return,
        // Coroutine expansion supplies the poll loop after the frame receives its native resume arguments.
        TerminatorKind::Await(..) => mir::TerminatorKind::Unreachable,
        TerminatorKind::CoroutineDrop => mir::TerminatorKind::CoroutineDrop,
        TerminatorKind::Unreachable => mir::TerminatorKind::Unreachable,
        TerminatorKind::UnwindResume => mir::TerminatorKind::UnwindResume,
    };
    Ok(mir::Terminator {
        source_info: mir::SourceInfo::outermost(span),
        kind,
        attributes: ThinVec::new(),
    })
}

/// Instantiate admitted runtime operations, clone calls and the retained lifetime binders of planned callees.
fn call_operand<'tcx>(
    tcx: TyCtxt<'tcx>,
    sources: &Sources<'_>,
    callee: &crate::plan::Callee,
) -> Result<mir::Operand<'tcx>, PlanError> {
    if let crate::plan::CalleeKind::Value(value) = &callee.kind {
        return operand(tcx, sources, value);
    }
    let (definition, arguments) = call_instance(tcx, callee)?;
    Ok(mir::Operand::function_handle(
        tcx,
        definition,
        arguments,
        sources.span(&callee.span)?,
    ))
}

/// Resolve one concrete call instance for both pre-MIR bound validation and MIR construction.
pub(crate) fn call_instance<'tcx>(
    tcx: TyCtxt<'tcx>,
    callee: &crate::plan::Callee,
) -> Result<(rustc_hir::def_id::DefId, rustc_middle::ty::GenericArgsRef<'tcx>), PlanError> {
    let mut arguments = Vec::new();
    match &callee.kind {
        crate::plan::CalleeKind::SpawnGenerator(name, leaf, depth) => {
            let producer = callees::resolve(
                tcx,
                &crate::plan::Callee {
                    kind: crate::plan::CalleeKind::Planned(name.clone()),
                    span: callee.span.clone(),
                },
            )?;
            arguments.push(generator_element(tcx, leaf, *depth)?.into());
            arguments.push(rustc_middle::ty::Ty::new_fn_def(tcx, producer, tcx.mk_args(&[])).into());
        }
        crate::plan::CalleeKind::YieldGenerator(leaf, depth)
        | crate::plan::CalleeKind::CollectGenerator(leaf, depth) => {
            arguments.push(generator_element(tcx, leaf, *depth)?.into());
        }
        _ => {}
    }
    if let crate::plan::CalleeKind::CallClosure(signature) = &callee.kind {
        let (_, parameters) = signature.split_last().ok_or_else(|| PlanError::Invalid {
            function: "callable object".into(),
            reason: "callable signature has no return type".into(),
        })?;
        arguments.push(crate::types::callable_object_type(tcx, signature)?.into());
        arguments.push(crate::types::callable_arguments_type(tcx, parameters)?.into());
    }
    if let crate::plan::CalleeKind::CallClosureOnce(signature) = &callee.kind {
        let (_, parameters) = signature.split_last().ok_or_else(|| PlanError::Invalid {
            function: "consuming callable object".into(),
            reason: "callable signature has no return type".into(),
        })?;
        let object = crate::types::consuming_callable_object_type(tcx, signature)?;
        arguments.push(rustc_middle::ty::Ty::new_box(tcx, object).into());
        arguments.push(crate::types::callable_arguments_type(tcx, parameters)?.into());
    }
    if let crate::plan::CalleeKind::CloneClosure(signature) = &callee.kind {
        arguments.push(crate::types::callable_object_type(tcx, signature)?.into());
    }
    if let crate::plan::CalleeKind::CloneEnum(_, name) = &callee.kind {
        arguments.push(crate::types::enum_type(tcx, name)?.into());
    }
    if let crate::plan::CalleeKind::CloneModel(_, name) = &callee.kind {
        arguments.push(crate::types::model_type(tcx, name)?.into());
    }
    if let crate::plan::CalleeKind::Instantiated(_, types) = &callee.kind {
        arguments.extend(callees::arguments(tcx, callees::resolve(tcx, callee)?, types)?.iter());
    }
    if let crate::plan::CalleeKind::FormatValue(ty, _) = &callee.kind {
        let definition = callees::resolve(tcx, callee)?;
        let closed = callees::arguments(tcx, definition, std::slice::from_ref(ty))?;
        callees::verify_bounds(tcx, definition, closed)?;
        arguments.extend(closed.iter());
    }
    if let crate::plan::CalleeKind::External(_) | crate::plan::CalleeKind::Planned(_) = &callee.kind {
        arguments.extend(callees::arguments(tcx, callees::resolve(tcx, callee)?, &[])?.iter());
    }
    Ok((callees::resolve(tcx, callee)?, tcx.mk_args(&arguments)))
}

/// Instantiate the runtime method's yielded element without changing its representation.
fn generator_element<'tcx>(
    tcx: TyCtxt<'tcx>,
    leaf: &crate::plan::ListLeaf,
    depth: i64,
) -> Result<rustc_middle::ty::Ty<'tcx>, PlanError> {
    let kind = if depth == 0 {
        crate::plan::list_leaf_type(leaf.clone())
    } else {
        crate::plan::PlanType::List(leaf.clone(), depth)
    };
    crate::types::native_type(tcx, &kind)
}

/// Supply the explicitly named producer function item to the runtime's generic spawn method.
fn generator_producer<'tcx>(
    tcx: TyCtxt<'tcx>,
    sources: &Sources<'_>,
    callee: &crate::plan::Callee,
) -> Result<Option<Spanned<mir::Operand<'tcx>>>, PlanError> {
    let crate::plan::CalleeKind::SpawnGenerator(name, _, _) = &callee.kind else {
        return Ok(None);
    };
    let producer = crate::plan::Callee {
        kind: crate::plan::CalleeKind::Planned(name.clone()),
        span: callee.span.clone(),
    };
    Ok(Some(Spanned {
        node: mir::Operand::function_handle(tcx, callees::resolve(tcx, &producer)?, [], sources.span(&callee.span)?),
        span: sources.span(&callee.span)?,
    }))
}
