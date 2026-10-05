//! Mapping admitted plan types to the pinned rustc representation.

use crate::error::PlanError;
use crate::plan::{PlanType, SizedNumeric};
use rustc_middle::ty::{Ty, TyCtxt};

/// Translate an admitted scalar or model type. Checked pairs and shared references are body-internal and never appear
/// in source signatures.
pub fn native_type<'tcx>(tcx: TyCtxt<'tcx>, ty: &PlanType) -> Result<Ty<'tcx>, PlanError> {
    Ok(match ty {
        PlanType::Model(_, name) => model_type(tcx, name)?,
        PlanType::ModelRef(_, name) => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, model_type(tcx, name)?),
        PlanType::Int => tcx.types.i64,
        PlanType::Float => tcx.types.f64,
        PlanType::I8 => tcx.types.i8,
        PlanType::I16 => tcx.types.i16,
        PlanType::I32 => tcx.types.i32,
        PlanType::I128 => tcx.types.i128,
        PlanType::U8 => tcx.types.u8,
        PlanType::U16 => tcx.types.u16,
        PlanType::U32 => tcx.types.u32,
        PlanType::U64 => tcx.types.u64,
        PlanType::U128 => tcx.types.u128,
        PlanType::F32 => tcx.types.f32,
        PlanType::F64 => tcx.types.f64,
        PlanType::CheckedNumeric(ty) => Ty::new_tup(tcx, &[numeric_type(tcx, ty), tcx.types.bool]),
        PlanType::ISize => tcx.types.isize,
        PlanType::USize => tcx.types.usize,
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

/// Resolve a previously injected source model; no external nominal can enter by spelling alone.
pub fn model_type<'tcx>(tcx: TyCtxt<'tcx>, name: &str) -> Result<Ty<'tcx>, PlanError> {
    let definition = tcx
        .hir_crate_items(())
        .free_items()
        .map(|item| item.owner_id.to_def_id())
        .find(|def| {
            tcx.def_kind(*def) == rustc_hir::def::DefKind::Struct
                && tcx.opt_item_name(*def).is_some_and(|symbol| symbol.as_str() == name)
        })
        .ok_or_else(|| PlanError::Invalid {
            function: name.into(),
            reason: "model declaration is missing".into(),
        })?;
    Ok(tcx.type_of(definition).instantiate_identity().skip_normalization())
}

/// Resolve the named sized carrier retained in an overflow pair.
fn numeric_type<'tcx>(tcx: TyCtxt<'tcx>, kind: &SizedNumeric) -> Ty<'tcx> {
    match kind {
        SizedNumeric::I8 => tcx.types.i8,
        SizedNumeric::I16 => tcx.types.i16,
        SizedNumeric::I32 => tcx.types.i32,
        SizedNumeric::I128 => tcx.types.i128,
        SizedNumeric::U8 => tcx.types.u8,
        SizedNumeric::U16 => tcx.types.u16,
        SizedNumeric::U32 => tcx.types.u32,
        SizedNumeric::U64 => tcx.types.u64,
        SizedNumeric::U128 => tcx.types.u128,
        SizedNumeric::F32 => tcx.types.f32,
        SizedNumeric::F64 => tcx.types.f64,
        SizedNumeric::ISize => tcx.types.isize,
        SizedNumeric::USize => tcx.types.usize,
    }
}
