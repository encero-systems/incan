//! A validated scalar plan becomes rustc MIR directly, with no THIR or generated Rust body.

use crate::error::PlanError;
use crate::plan::{Function, PlanType, Statement, StatementKind};
use crate::spans::Sources;
use crate::{terminators, types, values};
use rustc_hir::def_id::LocalDefId;
use rustc_index::IndexVec;
use rustc_middle::mir;
use rustc_middle::ty::TyCtxt;

/// Translate a validated statement, expanding native variant tests into a private discriminant read and comparison.
fn statement<'tcx>(
    tcx: TyCtxt<'tcx>,
    sources: &Sources<'_>,
    function: &Function,
    value: &Statement,
    locals: &mut IndexVec<mir::Local, mir::LocalDecl<'tcx>>,
) -> Result<Vec<mir::Statement<'tcx>>, PlanError> {
    let span = sources.span(&value.span)?;
    if let StatementKind::Assign(destination, rvalue) = &value.kind
        && let crate::plan::RvalueKind::IsVariant(source, variant) = &rvalue.kind
    {
        return variant_test(tcx, function, span, destination, source, *variant, locals);
    }
    let kind = match &value.kind {
        StatementKind::Assign(destination, value) => {
            let ty = match &destination.projection {
                crate::plan::Projection::Field(_, ty)
                | crate::plan::Projection::DerefField(_, ty)
                | crate::plan::Projection::Deref(ty) => ty,
                crate::plan::Projection::Fields(fields) | crate::plan::Projection::DerefFields(fields) => {
                    &fields
                        .last()
                        .ok_or_else(|| PlanError::Invalid {
                            function: function.name.clone(),
                            reason: "field path must retain at least one field".into(),
                        })?
                        .ty
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
    Ok(vec![mir::Statement::new(mir::SourceInfo::outermost(span), kind)])
}

/// Compare an enum's discriminant in its own native representation, using rustc's selected variant value. The
/// internal temporary never crosses the plan boundary; source enums, intrinsic carriers and foreign enums share it.
fn variant_test<'tcx>(
    tcx: TyCtxt<'tcx>,
    function: &Function,
    span: rustc_span::Span,
    destination: &crate::plan::Place,
    source: &crate::plan::Place,
    variant: i64,
    locals: &mut IndexVec<mir::Local, mir::LocalDecl<'tcx>>,
) -> Result<Vec<mir::Statement<'tcx>>, PlanError> {
    let invalid = |reason: &str| PlanError::Invalid {
        function: function.name.clone(),
        reason: reason.into(),
    };
    let source = values::place(tcx, source)?;
    let rustc_middle::ty::Adt(owner, _) = source.ty(locals, tcx).ty.kind() else {
        return Err(invalid("variant test requires native enum storage"));
    };
    let index = rustc_abi::VariantIdx::from_usize(values::index(variant)?);
    if owner.variants().get(index).is_none() {
        return Err(invalid("variant test index exceeds native enum layout"));
    }
    let discriminant = owner.discriminant_for_variant(tcx, index);
    let layout = tcx
        .layout_of(rustc_middle::ty::TypingEnv::fully_monomorphized().as_query_input(discriminant.ty))
        .map_err(|_| invalid("variant discriminant has no native layout"))?;
    let raw = locals.push(mir::LocalDecl::new(discriminant.ty, span));
    let expected = mir::Operand::Constant(Box::new(mir::ConstOperand {
        span,
        user_ty: None,
        const_: mir::Const::Val(
            mir::ConstValue::Scalar(mir::interpret::Scalar::from_uint(discriminant.val, layout.size)),
            discriminant.ty,
        ),
    }));
    let info = mir::SourceInfo::outermost(span);
    Ok(vec![
        mir::Statement::new(
            info,
            mir::StatementKind::Assign(Box::new((mir::Place::from(raw), mir::Rvalue::Discriminant(source)))),
        ),
        mir::Statement::new(
            info,
            mir::StatementKind::Assign(Box::new((
                values::place(tcx, destination)?,
                mir::Rvalue::BinaryOp(
                    mir::BinOp::Eq,
                    Box::new((mir::Operand::Copy(mir::Place::from(raw)), expected)),
                ),
            ))),
        ),
    ])
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
    let mut locals = IndexVec::from_raw(locals);
    let blocks = function
        .blocks
        .iter()
        .map(|block| {
            let mut statements = Vec::new();
            for value in &block.statements {
                statements.extend(statement(tcx, &sources, function, value, &mut locals)?);
            }
            Ok(mir::BasicBlockData::new_stmts(
                statements,
                Some(terminators::terminator(tcx, &sources, function, &block.terminator)?),
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
        locals,
        IndexVec::new(),
        function.parameters.len(),
        vec![],
        span,
        None,
        None,
    ))
}
