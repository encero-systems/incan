//! Mapping the scalar plan's types to the pinned rustc representation.

use crate::plan::PlanType;
use crate::error::PlanError;
use rustc_middle::ty::{Ty, TyCtxt};

/// Translate a source-owned scalar type. Checked pairs are body-internal and never appear in signatures.
pub fn native_type<'tcx>(tcx: TyCtxt<'tcx>, ty: &PlanType) -> Result<Ty<'tcx>, PlanError> {
    Ok(match ty {
        PlanType::Int => tcx.types.i64,
        PlanType::Float => tcx.types.f64,
        PlanType::Bool => tcx.types.bool,
        PlanType::Unit => tcx.types.unit,
        PlanType::CheckedInt => Ty::new_tup(tcx, &[tcx.types.i64, tcx.types.bool]),
        PlanType::String => string_type(tcx)?,
        PlanType::StringRef => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, string_type(tcx)?),
        PlanType::StrRef => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, tcx.types.str_),
        PlanType::StringArray(count) => array_type(tcx, string_type(tcx)?, *count)?,
        PlanType::StrArray(count) => array_type(tcx, native_type(tcx, &PlanType::StrRef)?, *count)?,
        PlanType::StringArrayRef(count) => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, array_type(tcx, string_type(tcx)?, *count)?),
        PlanType::StrArrayRef(count) => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, array_type(tcx, native_type(tcx, &PlanType::StrRef)?, *count)?),
        PlanType::StringSlice => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, Ty::new_slice(tcx, string_type(tcx)?)),
        PlanType::StrSlice => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, Ty::new_slice(tcx, native_type(tcx, &PlanType::StrRef)?)),
    })
}

/// Resolve the real standard String definition rather than manufacturing an ADT layout.
fn string_type(tcx: TyCtxt<'_>) -> Result<Ty<'_>, PlanError> {
    let definition = tcx.lang_items().string().ok_or_else(|| PlanError::Invalid {
        function: "String".into(), reason: "the native dependency closure has no String language item".into(),
    })?;
    Ok(tcx.type_of(definition).instantiate_identity().skip_normalization())
}

/// Translate a checked exact array length without a signed or truncating conversion.
fn array_type<'tcx>(tcx: TyCtxt<'tcx>, element: Ty<'tcx>, count: i64) -> Result<Ty<'tcx>, PlanError> {
    let count = u64::try_from(count).map_err(|_| PlanError::Invalid {
        function: "array".into(), reason: "array length must be nonnegative".into(),
    })?;
    Ok(Ty::new_array(tcx, element, count))
}
