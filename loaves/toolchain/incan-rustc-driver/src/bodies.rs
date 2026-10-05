//! A validated scalar plan becomes rustc MIR directly, with no THIR or generated Rust body.

use crate::error::PlanError;
use crate::plan::{Function, Statement, StatementKind};
use crate::spans::Sources;
use crate::{terminators, types, values};
use rustc_hir::def_id::LocalDefId;
use rustc_index::IndexVec;
use rustc_middle::mir;
use rustc_middle::ty::TyCtxt;

/// Translate one statement after all local and expression references have passed preflight validation.
fn statement<'tcx>(
    tcx: TyCtxt<'tcx>,
    sources: &Sources<'_>,
    function: &Function,
    value: &Statement,
) -> Result<mir::Statement<'tcx>, PlanError> {
    let location = match &value.kind {
        StatementKind::Assign(_, value) => &value.span,
        _ => &value.span,
    };
    let span = sources.span(location)?;
    let kind = match &value.kind {
        StatementKind::Assign(destination, value) => {
            let ty = &function.locals[values::index(destination.local)?].ty;
            mir::StatementKind::Assign(Box::new((
                values::place(tcx, destination)?,
                values::rvalue(tcx, sources, &value.kind, ty)?,
            )))
        }
        StatementKind::StorageLive(index) => {
            mir::StatementKind::StorageLive(mir::Local::from_usize(values::index(*index)?))
        }
        StatementKind::StorageDead(index) => {
            mir::StatementKind::StorageDead(mir::Local::from_usize(values::index(*index)?))
        }
    };
    Ok(mir::Statement::new(mir::SourceInfo::outermost(span), kind))
}

/// Supply a function's complete MIR body at the `mir_built` boundary.
pub fn body<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId, function: &Function) -> Result<mir::Body<'tcx>, PlanError> {
    let sources = Sources::new(tcx.sess.source_map());
    let span = sources.span(&function.span)?;
    let locals = function
        .locals
        .iter()
        .map(|local| {
            Ok(mir::LocalDecl::new(
                types::native_type(tcx, &local.ty)?,
                sources.span(&local.span)?,
            ))
        })
        .collect::<Result<Vec<_>, PlanError>>()?;
    let blocks = function
        .blocks
        .iter()
        .map(|block| {
            let statements = block
                .statements
                .iter()
                .map(|value| statement(tcx, &sources, function, value))
                .collect::<Result<_, _>>()?;
            Ok(mir::BasicBlockData::new_stmts(
                statements,
                Some(terminators::terminator(tcx, &sources, &block.terminator)?),
                block.cleanup,
            ))
        })
        .collect::<Result<Vec<_>, PlanError>>()?;
    let scopes = IndexVec::from_raw(vec![mir::SourceScopeData {
        span,
        parent_scope: None,
        inlined: None,
        inlined_parent_scope: None,
        local_data: mir::ClearCrossCrate::Clear,
    }]);
    Ok(mir::Body::new(
        mir::MirSource::item(def.to_def_id()),
        IndexVec::from_raw(blocks),
        scopes,
        IndexVec::from_raw(locals),
        IndexVec::new(),
        function.parameters.len(),
        vec![],
        span,
        None,
        None,
    ))
}
