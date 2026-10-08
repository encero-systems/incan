//! A validated scalar plan becomes rustc MIR directly, with no THIR or generated Rust body.

use crate::error::PlanError;
use crate::plan::{Function, PlanType, Statement, StatementKind};
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
            let ty = match &destination.projection {
                crate::plan::Projection::Field(_, ty)
                | crate::plan::Projection::DerefField(_, ty)
                | crate::plan::Projection::Deref(ty) => ty,
                crate::plan::Projection::Fields(fields) | crate::plan::Projection::DerefFields(fields) => {
                    &fields.last().ok_or_else(|| PlanError::Invalid {
                        function: function.name.clone(),
                        reason: "field path must retain at least one field".into(),
                    })?.ty
                }
                _ => &function.locals[values::index(destination.local)?].ty,
            };
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
            let span = sources.span(&local.span)?;
            let mut declaration = mir::LocalDecl::new(types::native_type(tcx, &local.ty)?, span);
            // Owned nominal slots retain runtime storage identity. Treating them as anonymous constant temps lets
            // PromoteTemps replace a later shared borrow with the initial value despite an intervening mutable call.
            if matches!(local.ty, PlanType::Model(..)) {
                declaration.local_info = mir::ClearCrossCrate::Set(Box::new(mir::LocalInfo::User(
                    mir::BindingForm::Var(mir::VarBindingForm {
                        binding_mode: rustc_hir::BindingMode(rustc_hir::ByRef::No, rustc_ast::Mutability::Mut),
                        opt_ty_info: None,
                        opt_match_place: None,
                        pat_span: span,
                        introductions: Vec::new(),
                    }),
                )));
            }
            Ok(declaration)
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
