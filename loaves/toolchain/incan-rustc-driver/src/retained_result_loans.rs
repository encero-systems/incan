//! Restore the checked source loan retained inside a top-level owned native result.

use crate::error::PlanError;
use rustc_middle::ty::{self, Ty, TyCtxt, TypeFoldable, TypeFolder, TypeSuperFoldable, TypeVisitable, TypeVisitor};
use std::ops::ControlFlow;

/// Tie metadata-erased result regions to the selected parameter's outer borrow. Results without native loans keep
/// their ordinary owned signature, while malformed plans refuse instead of inventing a source relationship.
pub(crate) fn signature<'tcx>(
    tcx: TyCtxt<'tcx>,
    signature: ty::PolyFnSig<'tcx>,
    source_slot: i64,
) -> Result<ty::PolyFnSig<'tcx>, PlanError> {
    if source_slot < 0 {
        return Ok(signature);
    }
    let slot = usize::try_from(source_slot).map_err(|_| invalid("negative retained result source slot"))?;
    let raw = signature.skip_binder();
    let source = raw
        .inputs()
        .get(slot)
        .ok_or_else(|| invalid("retained result source slot exceeds its signature"))?;
    let ty::Ref(source_region, _, _) = source.kind() else {
        return if has_erased_regions(raw.output()) {
            Err(invalid("retained native result source is not a borrowed parameter"))
        } else {
            Ok(signature)
        };
    };

    let mut bound = signature.bound_vars().to_vec();
    let source_region = if source_region.is_erased() {
        let var = ty::BoundVar::from_usize(bound.len());
        let kind = ty::BoundRegionKind::Anon;
        bound.push(ty::BoundVariableKind::Region(kind));
        ty::Region::new_bound(tcx, ty::INNERMOST, ty::BoundRegion { var, kind })
    } else {
        *source_region
    };
    let mut result_loans = ResultLoans {
        tcx,
        source: source_region,
        replaced: 0,
    };
    let output = raw.output().fold_with(&mut result_loans);
    if result_loans.replaced == 0 {
        return Ok(signature);
    }

    let mut types = raw.inputs_and_output.to_vec();
    if let ty::Ref(region, referent, mutability) = types[slot].kind()
        && region.is_erased()
    {
        types[slot] = Ty::new_ref(tcx, source_region, *referent, *mutability);
    }
    let Some(result) = types.last_mut() else {
        return Err(invalid("retained result signature has no result slot"));
    };
    *result = output;
    Ok(ty::Binder::bind_with_vars(
        ty::FnSig {
            inputs_and_output: tcx.mk_type_list(&types),
            ..raw
        },
        tcx.mk_bound_variable_kinds(&bound),
    ))
}

/// Detect whether the owned result needs a restored native loan before changing its source parameter.
fn has_erased_regions(ty: Ty<'_>) -> bool {
    ty.visit_with(&mut ErasedRegions).is_break()
}

/// Stop at the first metadata-erased region without entering independently bound callable signatures.
struct ErasedRegions;

impl<'tcx> TypeVisitor<TyCtxt<'tcx>> for ErasedRegions {
    type Result = ControlFlow<()>;

    fn visit_ty(&mut self, value: Ty<'tcx>) -> Self::Result {
        if matches!(value.kind(), ty::FnPtr(..)) {
            return ControlFlow::Continue(());
        }
        rustc_middle::ty::TypeSuperVisitable::super_visit_with(&value, self)
    }

    fn visit_region(&mut self, region: ty::Region<'tcx>) -> Self::Result {
        if region.is_erased() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}

/// Replace only erased native result regions; function-pointer binders remain independent call contracts.
struct ResultLoans<'tcx> {
    tcx: TyCtxt<'tcx>,
    source: ty::Region<'tcx>,
    replaced: usize,
}

impl<'tcx> TypeFolder<TyCtxt<'tcx>> for ResultLoans<'tcx> {
    /// Supply the native type interner.
    fn cx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }

    /// Keep nested callable signatures inside their own lifetime binders.
    fn fold_ty(&mut self, value: Ty<'tcx>) -> Ty<'tcx> {
        if matches!(value.kind(), ty::FnPtr(..)) {
            return value;
        }
        value.super_fold_with(self)
    }

    /// Preserve explicit regions and restore only metadata-erased loans in the owned result.
    fn fold_region(&mut self, region: ty::Region<'tcx>) -> ty::Region<'tcx> {
        if region.is_erased() {
            self.replaced += 1;
            self.source
        } else {
            region
        }
    }
}

/// Report malformed retained-loan metadata as a native-plan refusal.
fn invalid(reason: &str) -> PlanError {
    PlanError::Invalid {
        function: "retained result loan".into(),
        reason: reason.into(),
    }
}
