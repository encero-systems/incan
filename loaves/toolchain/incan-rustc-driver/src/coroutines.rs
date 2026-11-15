//! Planned async bodies enter before rustc's native coroutine transform.

use crate::error::PlanError;
use crate::plan::Function;
use crate::{bodies, types};
use rustc_hir::def_id::LocalDefId;
use rustc_index::IndexVec;
use rustc_middle::mir::visit::{MutVisitor, PlaceContext};
use rustc_middle::ty::TyCtxt;
use rustc_middle::{mir, ty};

/// Reserve the coroutine environment and resume context before the ordinary planned frame.
struct FrameShift<'tcx> {
    tcx: TyCtxt<'tcx>,
}

impl<'tcx> MutVisitor<'tcx> for FrameShift<'tcx> {
    /// Supply the active native context when the visitor interns shifted place projections.
    fn tcx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }
    /// Keep the return slot fixed while moving every source parameter and temporary past the coroutine arguments.
    fn visit_local(&mut self, local: &mut mir::Local, _context: PlaceContext, _location: mir::Location) {
        if *local != mir::RETURN_PLACE {
            *local = mir::Local::from_usize(local.as_usize() + 2);
        }
    }
}

/// Native MIR stores erased regions even when an AST alias explicitly owns a static trait object.
struct EraseRegions<'tcx> {
    tcx: TyCtxt<'tcx>,
}

impl<'tcx> MutVisitor<'tcx> for EraseRegions<'tcx> {
    /// Supply the native context for interned place and type updates.
    fn tcx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }

    /// Preserve representation while giving coroutine witness collection the erased-region MIR it requires.
    fn visit_ty(&mut self, ty: &mut ty::Ty<'tcx>, _context: rustc_middle::mir::visit::TyContext) {
        *ty = self.tcx.erase_and_anonymize_regions(*ty);
    }
}

/// Replace only executable blocks; native async declarations retain their opaque future and capture identities.
pub(crate) fn body<'tcx>(
    tcx: TyCtxt<'tcx>,
    def: LocalDefId,
    function: &Function,
) -> Result<mir::Body<'tcx>, PlanError> {
    let template = (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.mir_built)(tcx, def).borrow();
    let mut body = bodies::body(tcx, def, function)?;
    FrameShift { tcx }.visit_body(&mut body);
    let mut locals = IndexVec::new();
    locals.push(body.local_decls[mir::RETURN_PLACE].clone());
    locals.push(template.local_decls[mir::Local::from_usize(1)].clone());
    locals.push(template.local_decls[mir::Local::from_usize(2)].clone());
    for local in body.local_decls.iter().skip(1) {
        locals.push(local.clone());
    }
    body.local_decls = locals;
    body.arg_count = 2;
    body.coroutine = template.coroutine.clone();
    body.source = template.source;
    let ty::Coroutine(_, arguments) = body.local_decls[mir::Local::from_usize(1)].ty.kind() else {
        return Err(invalid(
            function,
            "unsupported async environment outside a native coroutine",
        ));
    };
    let captures = arguments.as_coroutine().tupled_upvars_ty().tuple_fields();
    if captures.len() != function.parameters.len() {
        return Err(invalid(
            function,
            "unsupported async capture count differs from the checked parameter frame",
        ));
    }
    let span = body.span;
    let mut statements = Vec::new();
    for (index, parameter) in function.parameters.iter().enumerate() {
        let expected = types::native_type(tcx, &parameter.ty)?;
        if tcx.erase_and_anonymize_regions(captures[index]) != tcx.erase_and_anonymize_regions(expected) {
            return Err(invalid(
                function,
                "unsupported async capture type differs from the checked parameter",
            ));
        }
        let source = mir::Place::from(mir::Local::from_usize(1)).project_deeper(
            &[mir::PlaceElem::Field(rustc_abi::FieldIdx::from_usize(index), expected)],
            tcx,
        );
        let destination = mir::Local::from_usize(index + 3);
        statements.push(mir::Statement::new(
            mir::SourceInfo::outermost(span),
            mir::StatementKind::StorageLive(destination),
        ));
        statements.push(mir::Statement::new(
            mir::SourceInfo::outermost(span),
            mir::StatementKind::Assign(Box::new((
                destination.into(),
                mir::Rvalue::Use(mir::Operand::Move(source), mir::WithRetag::Yes),
            ))),
        ));
    }
    body.basic_blocks.as_mut()[mir::START_BLOCK]
        .statements
        .splice(0..0, statements);
    expand_suspensions(tcx, &mut body, function)?;
    EraseRegions { tcx }.visit_body(&mut body);
    Ok(body)
}

/// Expand only the awaits explicitly selected by Incan lowering, after its source frame has been shifted.
fn expand_suspensions<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    function: &Function,
) -> Result<(), PlanError> {
    let sources = crate::spans::Sources::new(tcx.sess.source_map());
    for (index, block) in function.blocks.iter().enumerate() {
        if let crate::plan::TerminatorKind::Await(future, destination, continuation, cancelled, unwind, drop_unwind) =
            &block.terminator.kind
        {
            let mut future = crate::values::place(tcx, future)?;
            let mut destination = crate::values::place(tcx, destination)?;
            FrameShift { tcx }.visit_place(
                &mut future,
                PlaceContext::NonMutatingUse(rustc_middle::mir::visit::NonMutatingUseContext::PlaceMention),
                mir::Location::START,
            );
            FrameShift { tcx }.visit_place(
                &mut destination,
                PlaceContext::NonMutatingUse(rustc_middle::mir::visit::NonMutatingUseContext::PlaceMention),
                mir::Location::START,
            );
            expand_await(
                tcx,
                body,
                mir::BasicBlock::from_usize(index),
                AwaitSite {
                    future,
                    destination,
                    continuation: mir::BasicBlock::from_usize(crate::values::index(*continuation)?),
                    cancelled: mir::BasicBlock::from_usize(crate::values::index(*cancelled)?),
                    unwind: crate::terminators::unwind(unwind)?,
                    drop_unwind: crate::terminators::unwind(drop_unwind)?,
                    span: sources.span(&block.terminator.span)?,
                },
            )?;
        }
    }
    Ok(())
}

/// Preserve a named refusal when native capture metadata cannot realize the planned body exactly.
fn invalid(function: &Function, reason: &str) -> PlanError {
    PlanError::Invalid {
        function: function.name.clone(),
        reason: reason.to_owned(),
    }
}

/// The native declarations a suspension's pin, context and poll calls require.
struct PollTypes<'tcx> {
    pin: rustc_hir::def_id::DefId,
    context: rustc_hir::def_id::DefId,
    poll: rustc_hir::def_id::DefId,
    borrowed: ty::Ty<'tcx>,
    pinned: ty::Ty<'tcx>,
    context_ref: ty::Ty<'tcx>,
    polled: ty::Ty<'tcx>,
}

/// Prove the closed Future::poll predicates before normalizing its output and emitting a poll loop. Conditional
/// metadata impl templates alone cannot authorize a native future or reach borrow checking with an invalid call.
fn poll_types<'tcx>(tcx: TyCtxt<'tcx>, future: ty::Ty<'tcx>) -> Result<PollTypes<'tcx>, PlanError> {
    let pin_owner = tcx
        .lang_items()
        .pin_type()
        .ok_or_else(|| PlanError::UnknownCallee("unsupported Pin language item".into()))?;
    let pin = tcx
        .inherent_impls(pin_owner)
        .iter()
        .flat_map(|implementation| tcx.associated_item_def_ids(*implementation))
        .copied()
        .find(|item| tcx.item_name(*item).as_str() == "new_unchecked")
        .ok_or_else(|| PlanError::UnknownCallee("unsupported Pin::new_unchecked".into()))?;
    let context = tcx
        .lang_items()
        .get_context_fn()
        .ok_or_else(|| PlanError::UnknownCallee("unsupported async resume context".into()))?;
    let poll = tcx
        .lang_items()
        .future_poll_fn()
        .ok_or_else(|| PlanError::UnknownCallee("unsupported Future::poll".into()))?;
    crate::callees::verify_bounds(tcx, poll, tcx.mk_args(&[future.into()]))?;
    let borrowed = ty::Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, future);
    let pinned = ty::Ty::new_adt(tcx, tcx.adt_def(pin_owner), tcx.mk_args(&[borrowed.into()]));
    let context_args = context_arguments(tcx, context);
    let context_ref = tcx.normalize_erasing_regions(
        ty::TypingEnv::fully_monomorphized(),
        tcx.fn_sig(context)
            .instantiate(tcx, context_args)
            .map(|signature| tcx.instantiate_bound_regions_with_erased(signature).output()),
    );
    let polled = tcx.normalize_erasing_regions(
        ty::TypingEnv::fully_monomorphized(),
        tcx.fn_sig(poll)
            .instantiate(tcx, tcx.mk_args(&[future.into()]))
            .map(|signature| tcx.instantiate_bound_regions_with_erased(signature).output()),
    );
    Ok(PollTypes {
        pin,
        context,
        poll,
        borrowed,
        pinned,
        context_ref,
        polled,
    })
}

/// Resume context carries only native lifetime arguments; erased regions preserve its MIR calling convention.
fn context_arguments<'tcx>(tcx: TyCtxt<'tcx>, context: rustc_hir::def_id::DefId) -> ty::GenericArgsRef<'tcx> {
    ty::GenericArgs::for_item(tcx, context, |_, _| tcx.lifetimes.re_erased.into())
}

/// Add a suspension-only temporary; StorageLive/Dead keep the context borrow out of the saved coroutine frame.
fn temporary<'tcx>(body: &mut mir::Body<'tcx>, ty: ty::Ty<'tcx>, span: rustc_span::Span) -> mir::Local {
    body.local_decls.push(mir::LocalDecl::new(ty, span))
}

/// Build one native call block under the coroutine's current source span.
fn call_block<'tcx>(
    tcx: TyCtxt<'tcx>,
    function: rustc_hir::def_id::DefId,
    types: ty::GenericArgsRef<'tcx>,
    arguments: Vec<mir::Operand<'tcx>>,
    destination: mir::Place<'tcx>,
    next: mir::BasicBlock,
    unwind: mir::UnwindAction,
    statements: Vec<mir::Statement<'tcx>>,
    span: rustc_span::Span,
) -> mir::BasicBlockData<'tcx> {
    terminated(
        statements,
        mir::TerminatorKind::Call {
            func: mir::Operand::function_handle(tcx, function, types, span),
            args: arguments
                .into_iter()
                .map(|node| rustc_span::Spanned { node, span })
                .collect(),
            destination,
            target: Some(next),
            unwind,
            call_source: mir::CallSource::Normal,
            fn_span: span,
        },
        span,
    )
}

/// Preserve native source information on each expanded coroutine edge.
fn terminated<'tcx>(
    statements: Vec<mir::Statement<'tcx>>,
    kind: mir::TerminatorKind<'tcx>,
    span: rustc_span::Span,
) -> mir::BasicBlockData<'tcx> {
    mir::BasicBlockData::new_stmts(
        statements,
        Some(mir::Terminator {
            source_info: mir::SourceInfo::outermost(span),
            kind,
            attributes: thin_vec::ThinVec::new(),
        }),
        false,
    )
}

/// Retain the source-owned suspension destinations while native poll blocks are expanded.
struct AwaitSite<'tcx> {
    future: mir::Place<'tcx>,
    destination: mir::Place<'tcx>,
    continuation: mir::BasicBlock,
    cancelled: mir::BasicBlock,
    unwind: mir::UnwindAction,
    drop_unwind: mir::UnwindAction,
    span: rustc_span::Span,
}

/// Poll's native discriminants and payload layout are resolved from language items rather than guessed.
struct PollResult<'tcx> {
    ready: rustc_abi::VariantIdx,
    output: ty::Ty<'tcx>,
    ready_discriminant: u128,
    pending_discriminant: u128,
}

/// Require Future's native Poll carrier and the exact destination type before admitting either suspension edge.
fn poll_result<'tcx>(
    tcx: TyCtxt<'tcx>,
    polled: ty::Ty<'tcx>,
    destination: ty::Ty<'tcx>,
) -> Result<PollResult<'tcx>, PlanError> {
    let ty::Adt(owner, args) = polled.kind() else {
        return Err(PlanError::UnknownCallee(
            "unsupported Future::poll output outside Poll".into(),
        ));
    };
    let ready_id = tcx
        .lang_items()
        .poll_ready_variant()
        .ok_or_else(|| PlanError::UnknownCallee("unsupported Poll::Ready".into()))?;
    let pending_id = tcx
        .lang_items()
        .poll_pending_variant()
        .ok_or_else(|| PlanError::UnknownCallee("unsupported Poll::Pending".into()))?;
    let ready = owner.variant_index_with_id(ready_id);
    let pending = owner.variant_index_with_id(pending_id);
    let output = tcx.normalize_erasing_regions(
        ty::TypingEnv::fully_monomorphized(),
        owner.variant(ready).fields[rustc_abi::FieldIdx::from_usize(0)].ty(tcx, args),
    );
    if output != tcx.erase_and_anonymize_regions(destination) {
        return Err(PlanError::UnknownCallee(
            "unsupported awaited output differs from the checked destination".into(),
        ));
    }
    Ok(PollResult {
        ready,
        output,
        ready_discriminant: owner.discriminant_for_variant(tcx, ready).val,
        pending_discriminant: owner.discriminant_for_variant(tcx, pending).val,
    })
}

/// Expand a planned await after the ordinary frame has moved past the two native coroutine arguments.
fn expand_await<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    block: mir::BasicBlock,
    site: AwaitSite<'tcx>,
) -> Result<(), PlanError> {
    let AwaitSite {
        future,
        destination,
        span,
        ..
    } = site;
    let future_ty = future.ty(&body.local_decls, tcx).ty;
    let types = poll_types(tcx, future_ty)?;
    let result = poll_result(tcx, types.polled, destination.ty(&body.local_decls, tcx).ty)?;
    let borrowed = temporary(body, types.borrowed, span);
    let pinned = temporary(body, types.pinned, span);
    let context = temporary(body, types.context_ref, span);
    let polled = temporary(body, types.polled, span);
    let tag = temporary(body, tcx.types.isize, span);
    let base = body.basic_blocks.len();
    let pin_block = mir::BasicBlock::from_usize(base);
    let context_block = mir::BasicBlock::from_usize(base + 1);
    let poll_block = mir::BasicBlock::from_usize(base + 2);
    let select_block = mir::BasicBlock::from_usize(base + 3);
    let ready_block = mir::BasicBlock::from_usize(base + 4);
    let pending_block = mir::BasicBlock::from_usize(base + 5);
    let invalid_block = mir::BasicBlock::from_usize(base + 6);
    body.basic_blocks.as_mut()[block].terminator_mut().kind = mir::TerminatorKind::Goto { target: pin_block };
    let info = mir::SourceInfo::outermost(span);
    let assign = |place, rvalue| mir::Statement::new(info, mir::StatementKind::Assign(Box::new((place, rvalue))));
    let mut statements = [borrowed, pinned, context, polled, tag]
        .into_iter()
        .map(|local| mir::Statement::new(info, mir::StatementKind::StorageLive(local)))
        .collect::<Vec<_>>();
    statements.push(assign(
        borrowed.into(),
        mir::Rvalue::Ref(
            tcx.lifetimes.re_erased,
            mir::BorrowKind::Mut {
                kind: mir::MutBorrowKind::Default,
            },
            future,
        ),
    ));
    let pin_call = call_block(
        tcx,
        types.pin,
        tcx.mk_args(&[types.borrowed.into()]),
        vec![mir::Operand::Move(borrowed.into())],
        pinned.into(),
        context_block,
        site.unwind,
        statements,
        span,
    );
    body.basic_blocks.as_mut().push(pin_call);
    let context_call = call_block(
        tcx,
        types.context,
        context_arguments(tcx, types.context),
        vec![mir::Operand::Copy(mir::Local::from_usize(2).into())],
        context.into(),
        poll_block,
        site.unwind,
        Vec::new(),
        span,
    );
    body.basic_blocks.as_mut().push(context_call);
    let poll_call = call_block(
        tcx,
        types.poll,
        tcx.mk_args(&[future_ty.into()]),
        vec![mir::Operand::Move(pinned.into()), mir::Operand::Move(context.into())],
        polled.into(),
        select_block,
        site.unwind,
        Vec::new(),
        span,
    );
    body.basic_blocks.as_mut().push(poll_call);
    finish_poll(
        tcx,
        body,
        site,
        result,
        [borrowed, pinned, context, polled, tag],
        [
            pin_block,
            context_block,
            poll_block,
            select_block,
            ready_block,
            pending_block,
            invalid_block,
        ],
    );
    Ok(())
}

/// Move only a Ready payload, or release temporary borrows and yield the native resume context on Pending.
fn finish_poll<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    site: AwaitSite<'tcx>,
    result: PollResult<'tcx>,
    locals: [mir::Local; 5],
    edges: [mir::BasicBlock; 7],
) {
    let AwaitSite {
        future,
        destination,
        continuation,
        cancelled,
        span,
        unwind: _,
        drop_unwind,
    } = site;
    let PollResult {
        ready,
        output,
        ready_discriminant,
        pending_discriminant,
    } = result;
    let [borrowed, pinned, context, polled, tag] = locals;
    let [pin_block, _, _, _, ready_block, pending_block, invalid_block] = edges;
    let info = mir::SourceInfo::outermost(span);
    let assign = |place, rvalue| mir::Statement::new(info, mir::StatementKind::Assign(Box::new((place, rvalue))));
    let dead_borrows = [borrowed, pinned, context]
        .into_iter()
        .map(|local| mir::Statement::new(info, mir::StatementKind::StorageDead(local)));
    let mut statements = dead_borrows.collect::<Vec<_>>();
    statements.push(assign(tag.into(), mir::Rvalue::Discriminant(polled.into())));
    body.basic_blocks.as_mut().push(terminated(
        statements,
        mir::TerminatorKind::SwitchInt {
            discr: mir::Operand::Move(tag.into()),
            targets: mir::SwitchTargets::new(
                [(ready_discriminant, ready_block), (pending_discriminant, pending_block)].into_iter(),
                invalid_block,
            ),
        },
        span,
    ));
    let payload = mir::Place::from(polled).project_deeper(
        &[
            mir::PlaceElem::Downcast(None, ready),
            mir::PlaceElem::Field(rustc_abi::FieldIdx::from_usize(0), output),
        ],
        tcx,
    );
    let mut ready_statements = vec![assign(
        destination,
        mir::Rvalue::Use(mir::Operand::Move(payload), mir::WithRetag::Yes),
    )];
    ready_statements.extend(
        [polled, tag]
            .into_iter()
            .map(|local| mir::Statement::new(info, mir::StatementKind::StorageDead(local))),
    );
    body.basic_blocks.as_mut().push(terminated(
        ready_statements,
        mir::TerminatorKind::Drop {
            place: future,
            target: continuation,
            unwind: drop_unwind,
            replace: false,
            drop: None,
        },
        span,
    ));
    let pending_statements = [polled, tag]
        .into_iter()
        .map(|local| mir::Statement::new(info, mir::StatementKind::StorageDead(local)))
        .collect();
    let unit = mir::Operand::Constant(Box::new(mir::ConstOperand {
        span,
        user_ty: None,
        const_: mir::Const::Val(mir::ConstValue::ZeroSized, tcx.types.unit),
    }));
    body.basic_blocks.as_mut().push(terminated(
        pending_statements,
        mir::TerminatorKind::Yield {
            value: unit,
            resume: pin_block,
            resume_arg: mir::Local::from_usize(2).into(),
            drop: Some(cancelled),
        },
        span,
    ));
    body.basic_blocks
        .as_mut()
        .push(terminated(Vec::new(), mir::TerminatorKind::Unreachable, span));
}
