//! Mapping the scalar plan's types to the pinned rustc representation.

use crate::plan::PlanType;
use rustc_middle::ty::{Ty, TyCtxt};

/// Translate a source-owned scalar type. Checked pairs are body-internal and never appear in signatures.
pub fn native_type<'tcx>(tcx: TyCtxt<'tcx>, ty: &PlanType) -> Ty<'tcx> {
    match ty {
        PlanType::Int => tcx.types.i64,
        PlanType::Float => tcx.types.f64,
        PlanType::Bool => tcx.types.bool,
        PlanType::Unit => tcx.types.unit,
        PlanType::CheckedInt => Ty::new_tup(tcx, &[tcx.types.i64, tcx.types.bool]),
    }
}
