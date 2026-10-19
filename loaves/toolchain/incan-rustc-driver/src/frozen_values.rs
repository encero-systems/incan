//! Static literal storage for dependency-owned frozen collection carriers.

use crate::error::PlanError;
use crate::plan::{Constant, FrozenValue, PlanType};
use crate::types::native_type;
use rustc_abi::{FieldIdx, Size, VariantIdx};
use rustc_index::IndexVec;
use rustc_middle::mir::{
    self,
    interpret::{AllocId, AllocInit, Allocation, Pointer, Scalar, alloc_range},
};
use rustc_middle::ty::{self, Ty, TyCtxt};

/// Name unsupported static layouts before they can produce invalid MIR or uninitialized pointer data.
fn invalid(reason: impl Into<String>) -> PlanError {
    PlanError::Invalid {
        function: "frozen collection".into(),
        reason: format!("unsupported frozen collection {}", reason.into()),
    }
}

/// Build the actual dependency-owned wrapper over a fully initialized immutable static slice.
pub fn construct<'tcx>(
    tcx: TyCtxt<'tcx>,
    ty: &PlanType,
    values: &[FrozenValue],
) -> Result<mir::Rvalue<'tcx>, PlanError> {
    let carrier = native_type(tcx, ty)?;
    let (definition, args, field, element) = storage(tcx, carrier)?;
    let (alloc_id, meta) = allocate(tcx, element, values)?;
    let literal = mir::Operand::Constant(Box::new(mir::ConstOperand {
        span: rustc_span::DUMMY_SP,
        user_ty: None,
        const_: mir::Const::Val(mir::ConstValue::Slice { alloc_id, meta }, field),
    }));
    Ok(mir::Rvalue::Aggregate(
        Box::new(mir::AggregateKind::Adt(
            definition.did(),
            VariantIdx::from_usize(0),
            args,
            None,
            None,
        )),
        IndexVec::from_raw(vec![literal]),
    ))
}

/// Validate that linked metadata still describes a single static slice field; its element layout is authoritative.
fn storage<'tcx>(
    tcx: TyCtxt<'tcx>,
    carrier: Ty<'tcx>,
) -> Result<(ty::AdtDef<'tcx>, ty::GenericArgsRef<'tcx>, Ty<'tcx>, Ty<'tcx>), PlanError> {
    let ty::Adt(definition, args) = carrier.kind() else {
        return Err(invalid("carrier is not an ADT"));
    };
    if !definition.is_struct() || definition.non_enum_variant().fields.len() != 1 {
        return Err(invalid("carrier does not have one field"));
    }
    let field = definition.non_enum_variant().fields[FieldIdx::from_usize(0)]
        .ty(tcx, *args)
        .skip_normalization();
    let ty::Ref(region, pointee, rustc_hir::Mutability::Not) = field.kind() else {
        return Err(invalid("field is not shared storage"));
    };
    let ty::Slice(element) = pointee.kind() else {
        return Err(invalid("field is not a slice"));
    };
    if !region.is_static() {
        return Err(invalid("slice lifetime is not static"));
    }
    Ok((*definition, *args, field, *element))
}

/// Allocate source-ordered scalar, pair, or nested frozen elements with target-derived stride and alignment.
fn allocate<'tcx>(tcx: TyCtxt<'tcx>, element: Ty<'tcx>, values: &[FrozenValue]) -> Result<(AllocId, u64), PlanError> {
    let layout = tcx
        .layout_of(ty::TypingEnv::fully_monomorphized().as_query_input(element))
        .map_err(|error| invalid(format!("element layout: {error:?}")))?;
    let pair = matches!(element.kind(), ty::Tuple(fields) if fields.len() == 2);
    let width = if pair { 2 } else { 1 };
    if !values.len().is_multiple_of(width) {
        return Err(invalid("dictionary data is incomplete"));
    }
    let count = u64::try_from(values.len() / width).map_err(|_| invalid("literal length"))?;
    let size = layout
        .size
        .checked_mul(count, &tcx.data_layout)
        .ok_or_else(|| invalid("literal storage exceeds target limits"))?;
    let mut allocation = Allocation::new(size, layout.align.abi, AllocInit::Uninit, ());
    for (index, entries) in values.chunks(width).enumerate() {
        let index = u64::try_from(index).map_err(|_| invalid("literal index"))?;
        let offset = layout.size * index;
        if let ty::Tuple(fields) = element.kind() {
            for (slot, entry) in entries.iter().enumerate() {
                write(
                    tcx,
                    &mut allocation,
                    offset + layout.fields.offset(slot),
                    fields[slot],
                    entry,
                )?;
            }
        } else {
            write(tcx, &mut allocation, offset, element, &entries[0])?;
        }
    }
    allocation.mutability = rustc_hir::Mutability::Not;
    Ok((tcx.reserve_and_set_memory_alloc(tcx.mk_const_alloc(allocation)), count))
}

/// Write one literal according to its exact native type, including pointer provenance for static strings and slices.
fn write<'tcx>(
    tcx: TyCtxt<'tcx>,
    allocation: &mut Allocation,
    offset: Size,
    ty: Ty<'tcx>,
    value: &FrozenValue,
) -> Result<(), PlanError> {
    let layout = tcx
        .layout_of(ty::TypingEnv::fully_monomorphized().as_query_input(ty))
        .map_err(|error| invalid(format!("value layout: {error:?}")))?;
    match (ty.kind(), value) {
        (ty::Int(ty::IntTy::I64), FrozenValue::Scalar(Constant::Int(value))) => {
            put(tcx, allocation, offset, layout.size, Scalar::from_i64(*value))
        }
        (ty::Bool, FrozenValue::Scalar(Constant::Bool(value))) => {
            put(tcx, allocation, offset, layout.size, Scalar::from_bool(*value))
        }
        (ty::Adt(definition, args), FrozenValue::Scalar(Constant::Text(value)))
            if native_type(tcx, &PlanType::FrozenStr)? == ty =>
        {
            if !definition.is_struct() || definition.non_enum_variant().fields.len() != 1 {
                return Err(invalid("frozen text wrapper layout"));
            }
            let field = definition.non_enum_variant().fields[FieldIdx::from_usize(0)]
                .ty(tcx, *args)
                .skip_normalization();
            if !matches!(field.kind(), ty::Ref(region, inner, rustc_hir::Mutability::Not) if region.is_static() && inner.is_str())
            {
                return Err(invalid("frozen text field is not static text"));
            }
            let id = tcx.allocate_bytes_dedup(value.as_bytes(), mir::interpret::CTFE_ALLOC_SALT);
            fat_pointer(
                tcx,
                allocation,
                offset + layout.fields.offset(0),
                id,
                u64::try_from(value.len()).map_err(|_| invalid("frozen text length"))?,
            )
        }
        (ty::Ref(region, inner, rustc_hir::Mutability::Not), FrozenValue::Scalar(Constant::Text(value)))
            if region.is_static() && inner.is_str() =>
        {
            let id = tcx.allocate_bytes_dedup(value.as_bytes(), mir::interpret::CTFE_ALLOC_SALT);
            fat_pointer(
                tcx,
                allocation,
                offset,
                id,
                u64::try_from(value.len()).map_err(|_| invalid("text length"))?,
            )
        }
        (ty::Adt(..), FrozenValue::Bytes(_)) if native_type(tcx, &PlanType::FrozenBytes)? == ty => {
            let (_, _, field, element) = storage(tcx, ty)?;
            if element != tcx.types.u8 {
                return Err(invalid("frozen byte wrapper element"));
            }
            write(tcx, allocation, offset + layout.fields.offset(0), field, value)
        }
        (ty::Ref(region, inner, rustc_hir::Mutability::Not), FrozenValue::Bytes(values))
            if region.is_static() && matches!(inner.kind(), ty::Slice(element) if *element == tcx.types.u8) =>
        {
            let bytes = values
                .iter()
                .map(|value| u8::try_from(*value))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| invalid("non-byte literal"))?;
            let id = tcx.allocate_bytes_dedup(&bytes, mir::interpret::CTFE_ALLOC_SALT);
            fat_pointer(
                tcx,
                allocation,
                offset,
                id,
                u64::try_from(bytes.len()).map_err(|_| invalid("byte length"))?,
            )
        }
        (ty::Adt(..), FrozenValue::Collection(carrier, values)) if native_type(tcx, carrier)? == ty => {
            let (_, _, _, element) = storage(tcx, ty)?;
            let (id, count) = allocate(tcx, element, values)?;
            fat_pointer(tcx, allocation, offset + layout.fields.offset(0), id, count)
        }
        _ => Err(invalid("literal does not match its checked storage type")),
    }
}

/// Store the allocation pointer and element count using the target's pointer width, without erasing provenance.
fn fat_pointer(
    tcx: TyCtxt<'_>,
    allocation: &mut Allocation,
    offset: Size,
    id: AllocId,
    count: u64,
) -> Result<(), PlanError> {
    let width = tcx.data_layout.pointer_size();
    put(
        tcx,
        allocation,
        offset,
        width,
        Scalar::from_pointer(Pointer::new(id.into(), Size::ZERO), &tcx),
    )?;
    put(tcx, allocation, offset + width, width, Scalar::from_uint(count, width))
}

/// Write an already validated scalar at its target-derived offset, propagating allocation errors as named refusals.
fn put(tcx: TyCtxt<'_>, allocation: &mut Allocation, offset: Size, size: Size, value: Scalar) -> Result<(), PlanError> {
    allocation
        .write_scalar(&tcx, alloc_range(offset, size), value)
        .map_err(|error| invalid(format!("literal allocation: {error:?}")))
}
