//! Mapping admitted plan types to the pinned rustc representation.

use crate::error::PlanError;
use crate::plan::PlanType;
use rustc_middle::ty::{Ty, TyCtxt};

/// Translate an admitted scalar, model, or callable type. Checked pairs and shared references are body-internal and
/// never appear in source signatures.
pub fn native_type<'tcx>(tcx: TyCtxt<'tcx>, ty: &PlanType) -> Result<Ty<'tcx>, PlanError> {
    Ok(match ty {
        PlanType::Model(_, name) => model_type(tcx, name)?,
        PlanType::ModelRef(_, name) => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, model_type(tcx, name)?),
        PlanType::Int => tcx.types.i64,
        PlanType::Float => tcx.types.f64,
        PlanType::Bool => tcx.types.bool,
        PlanType::FunctionPointer(signature) => function_pointer_type(tcx, signature)?,
        PlanType::FunctionItem(name, _) => Ty::new_fn_def(tcx, crate::callees::planned(tcx, name)?, []),
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

/// Build a safe Rust-ABI function pointer from its checked inputs and final return entry.
fn function_pointer_type<'tcx>(tcx: TyCtxt<'tcx>, signature: &[PlanType]) -> Result<Ty<'tcx>, PlanError> {
    let (result, parameters) = signature.split_last().ok_or_else(|| PlanError::Invalid {
        function: "function pointer".into(),
        reason: "callable signature has no return type".into(),
    })?;
    let inputs = parameters
        .iter()
        .map(|ty| native_type(tcx, ty))
        .collect::<Result<Vec<_>, _>>()?;
    let signature = tcx.mk_fn_sig_safe_rust_abi(inputs, native_type(tcx, result)?);
    Ok(Ty::new_fn_ptr(tcx, rustc_middle::ty::Binder::dummy(signature)))
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
