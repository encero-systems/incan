//! Materialize checked native array-to-slice borrows through the canonical core view traits.

use incan_mir_plan_native::{
    CalleeKind, ExternalFunction, Operand, Place, PlanType, Projection, RvalueKind, SourceSpan, assign as plan_assign,
};
use incan_semantics_core::body_ir::OwnershipFact;

use crate::frame::{Builder, place_type, read_place, temporary, terminate_call};

/// Borrow only an exact checked array element carrier, retaining exclusive authority for mutable views.
pub fn borrow_array_slice(
    builder: &mut Builder,
    source: &Place,
    elements: &[PlanType],
    mutable: bool,
    fact: &OwnershipFact,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    let mut owner = source.clone();
    let mut already_shared = false;
    let (items, count) = match place_type(&builder.locals, source) {
        PlanType::NativeArray(items, count) => (items, count),
        PlanType::NativeArrayRef(items, count) => {
            if mutable {
                return Err("unsupported Body IR mutable slice through a shared array reference".into());
            }
            already_shared = true;
            (items, count)
        }
        PlanType::NativeArrayMutRef(items, count) => {
            owner = Place {
                local: source.local,
                projection: Projection::Deref(PlanType::NativeArray(items.clone(), count)),
                span: span.clone(),
            };
            (items, count)
        }
        _ => return Ok(None),
    };
    if items != elements {
        return Err("unsupported Body IR native array slice element mismatch".into());
    }
    if mutable && *fact != OwnershipFact::MutBorrow {
        return Err("unsupported Body IR native array slice without exclusive authority".into());
    }
    let (parameter, result, method, operation) = if mutable {
        (
            PlanType::NativeArrayMutRef(items.clone(), count),
            PlanType::SliceMutRef(elements.to_vec()),
            "core::convert::AsMut::as_mut",
            RvalueKind::MutBorrow(owner),
        )
    } else {
        (
            PlanType::NativeArrayRef(items.clone(), count),
            PlanType::SliceRef(elements.to_vec()),
            "core::convert::AsRef::as_ref",
            RvalueKind::Borrow(owner),
        )
    };
    let mut reference = read_place(source.clone(), false, span);
    if !already_shared {
        let borrowed = temporary(builder, parameter.clone(), span);
        builder.emit(plan_assign(borrowed.clone(), operation, span));
        reference = read_place(borrowed, mutable, span);
    }
    let output = temporary(builder, result.clone(), span);
    let arguments = vec![
        PlanType::NativeArray(items, count),
        PlanType::NativeSlice(elements.to_vec()),
    ];
    builder.externals.push(ExternalFunction {
        path: method.to_owned(),
        type_arguments: arguments.clone(),
        parameters: vec![parameter],
        return_type: result,
        span: span.clone(),
    });
    terminate_call(
        builder,
        CalleeKind::Instantiated(method.to_owned(), arguments),
        vec![reference],
        output.clone(),
        span,
    );
    Ok(Some(read_place(output, mutable, span)))
}
