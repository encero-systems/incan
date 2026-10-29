//! Call-scoped lifetime binders for opaque native inputs; storage remains region-erased in MIR.

use rustc_middle::ty::{self, Ty, TyCtxt, TypeFoldable, TypeFolder, TypeSuperFoldable};

/// Restore independently elided native input loans before signature outlives checking (#2368).
/// Existing source binders remain intact. Outputs are deliberately unchanged: an erased output cannot acquire a
/// relationship to an input merely because their native carriers happen to match.
pub(crate) fn signature<'tcx>(tcx: TyCtxt<'tcx>, signature: ty::PolyFnSig<'tcx>) -> ty::PolyFnSig<'tcx> {
    let mut loans = InputLoans {
        tcx,
        bound: signature.bound_vars().to_vec(),
    };
    let raw = signature.skip_binder();
    let mut types: Vec<_> = raw.inputs().iter().map(|input| input.fold_with(&mut loans)).collect();
    types.push(raw.output());
    ty::Binder::bind_with_vars(
        ty::FnSig {
            inputs_and_output: tcx.mk_type_list(&types),
            ..raw
        },
        tcx.mk_bound_variable_kinds(&loans.bound),
    )
}

/// Bind every metadata-erased input region, including a native ADT's own lifetime arguments.
pub(crate) fn inputs<'tcx>(
    tcx: TyCtxt<'tcx>,
    inputs: &[Ty<'tcx>],
) -> (Vec<Ty<'tcx>>, Vec<ty::BoundVariableKind<'tcx>>) {
    let mut loans = InputLoans { tcx, bound: Vec::new() };
    let inputs = inputs.iter().map(|input| input.fold_with(&mut loans)).collect();
    (inputs, loans.bound)
}

/// One independently bound input signature; nested callable signatures own separate binders.
struct InputLoans<'tcx> {
    tcx: TyCtxt<'tcx>,
    bound: Vec<ty::BoundVariableKind<'tcx>>,
}

impl<'tcx> TypeFolder<TyCtxt<'tcx>> for InputLoans<'tcx> {
    /// Supply the native interner for folded types and regions.
    fn cx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }

    /// Keep a function pointer's higher-ranked inputs inside its own signature binder.
    fn fold_ty(&mut self, value: Ty<'tcx>) -> Ty<'tcx> {
        if let ty::FnPtr(..) = value.kind() {
            return Ty::new_fn_ptr(self.tcx, signature(self.tcx, value.fn_sig(self.tcx)));
        }
        value.super_fold_with(self)
    }

    /// Preserve other independently quantified predicates instead of borrowing the enclosing function's binder.
    fn fold_binder<T: TypeFoldable<TyCtxt<'tcx>>>(&mut self, value: ty::Binder<'tcx, T>) -> ty::Binder<'tcx, T> {
        value
    }

    /// Metadata-erased occurrences in input position follow Rust's independent input lifetime elision.
    fn fold_region(&mut self, region: ty::Region<'tcx>) -> ty::Region<'tcx> {
        if !region.is_erased() {
            return region;
        }
        let var = ty::BoundVar::from_usize(self.bound.len());
        let kind = ty::BoundRegionKind::Anon;
        self.bound.push(ty::BoundVariableKind::Region(kind));
        ty::Region::new_bound(self.tcx, ty::INNERMOST, ty::BoundRegion { var, kind })
    }
}
