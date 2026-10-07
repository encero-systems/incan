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
fn unwind(value: &Unwind) -> Result<mir::UnwindAction, PlanError> {
    Ok(match value {
        Unwind::Continue => mir::UnwindAction::Continue,
        Unwind::Cleanup(target) => mir::UnwindAction::Cleanup(block(*target)?),
        Unwind::Terminate => mir::UnwindAction::Terminate(mir::UnwindTerminateReason::InCleanup),
    })
}

/// Lower one block exit, retaining its source location and call argument locations.
pub fn terminator<'tcx>(
    tcx: TyCtxt<'tcx>,
    sources: &Sources<'_>,
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
                .collect::<Result<_, PlanError>>()?,
            destination: place(tcx, destination)?,
            target: Some(block(*target)?),
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
        TerminatorKind::Unreachable => mir::TerminatorKind::Unreachable,
        TerminatorKind::UnwindResume => mir::TerminatorKind::UnwindResume,
    };
    Ok(mir::Terminator {
        source_info: mir::SourceInfo::outermost(span),
        kind,
        attributes: ThinVec::new(),
    })
}

/// Instantiate only the admitted model Clone call; ordinary callees remain monomorphic.
fn call_operand<'tcx>(
    tcx: TyCtxt<'tcx>,
    sources: &Sources<'_>,
    callee: &crate::plan::Callee,
) -> Result<mir::Operand<'tcx>, PlanError> {
    let mut arguments = Vec::new();
    if let crate::plan::CalleeKind::CloneModel(_, name) = &callee.kind {
        arguments.push(crate::types::model_type(tcx, name)?.into());
    }
    if let crate::plan::CalleeKind::Instantiated(_, ty) = &callee.kind {
        arguments.extend(callees::arguments(tcx, callees::resolve(tcx, callee)?, std::slice::from_ref(ty))?.iter());
    }
    Ok(mir::Operand::function_handle(
        tcx,
        callees::resolve(tcx, callee)?,
        arguments,
        sources.span(&callee.span)?,
    ))
}
