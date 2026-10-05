//! Mapping the scalar plan's types to the pinned rustc representation.

use crate::error::PlanError;
use crate::plan::PlanType;
use rustc_middle::ty::{Ty, TyCtxt};

/// Translate a source-owned scalar type. Checked pairs are body-internal and never appear in signatures.
pub fn native_type<'tcx>(tcx: TyCtxt<'tcx>, ty: &PlanType) -> Result<Ty<'tcx>, PlanError> {
    Ok(match ty {
        PlanType::List(leaf, depth) => list_type(tcx, *leaf, *depth)?,
        PlanType::ListRef(leaf, depth) => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, list_type(tcx, *leaf, *depth)?),
        PlanType::ListMutRef(leaf, depth) => {
            Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, list_type(tcx, *leaf, *depth)?)
        }
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
        PlanType::StringArrayRef(count) => Ty::new_imm_ref(
            tcx,
            tcx.lifetimes.re_erased,
            array_type(tcx, string_type(tcx)?, *count)?,
        ),
        PlanType::StrArrayRef(count) => Ty::new_imm_ref(
            tcx,
            tcx.lifetimes.re_erased,
            array_type(tcx, native_type(tcx, &PlanType::StrRef)?, *count)?,
        ),
        PlanType::StringSlice => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, Ty::new_slice(tcx, string_type(tcx)?)),
        PlanType::StrSlice => Ty::new_imm_ref(
            tcx,
            tcx.lifetimes.re_erased,
            Ty::new_slice(tcx, native_type(tcx, &PlanType::StrRef)?),
        ),
    })
}

/// Resolve the real standard String definition rather than manufacturing an ADT layout.
fn string_type(tcx: TyCtxt<'_>) -> Result<Ty<'_>, PlanError> {
    let definition = tcx.lang_items().string().ok_or_else(|| PlanError::Invalid {
        function: "String".into(),
        reason: "the native dependency closure has no String language item".into(),
    })?;
    Ok(tcx.type_of(definition).instantiate_identity().skip_normalization())
}

/// Translate a checked exact array length without a signed or truncating conversion.
fn array_type<'tcx>(tcx: TyCtxt<'tcx>, element: Ty<'tcx>, count: i64) -> Result<Ty<'tcx>, PlanError> {
    let count = u64::try_from(count).map_err(|_| PlanError::Invalid {
        function: "array".into(),
        reason: "array length must be nonnegative".into(),
    })?;
    Ok(Ty::new_array(tcx, element, count))
}

/// Select the primitive leaf and wrap it in the standard Vec ADT once per retained list dimension.
fn list_type<'tcx>(tcx: TyCtxt<'tcx>, leaf: i64, depth: i64) -> Result<Ty<'tcx>, PlanError> {
    let mut element = match leaf {
        0 => tcx.types.i64,
        1 => tcx.types.f64,
        2 => tcx.types.bool,
        3 => string_type(tcx)?,
        _ => {
            return Err(PlanError::Invalid {
                function: "list".into(),
                reason: "invalid primitive list leaf".into(),
            });
        }
    };
    if depth < 1 {
        return Err(PlanError::Invalid {
            function: "list".into(),
            reason: "list depth must be positive".into(),
        });
    }
    let definition = tcx
        .get_diagnostic_item(rustc_span::sym::Vec)
        .ok_or_else(|| PlanError::Invalid {
            function: "list".into(),
            reason: "native closure has no standard Vec definition".into(),
        })?;
    for _ in 0..depth {
        element = Ty::new_adt(
            tcx,
            tcx.adt_def(definition),
            rustc_middle::ty::GenericArgs::for_item(tcx, definition, |parameter, _| {
                if parameter.index == 0 {
                    element.into()
                } else {
                    tcx.type_of(parameter.def_id)
                        .instantiate_identity()
                        .skip_normalization()
                        .into()
                }
            }),
        );
    }
    Ok(element)
}
