//! Exact byte literal materialization through the legacy Vec[u8] representation.

use incan_mir_plan_native::{Constant, ListLeaf, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign};
use incan_semantics_core::body_ir::Constant as SourceConstant;

use crate::builtins::list_call;
use crate::frame::{Builder, literal, read_place, temporary};

/// Build an owned byte vector with exact unsigned constants, including empty and non-ASCII literals.
pub fn bytes_literal(builder: &mut Builder, value: &SourceConstant, span: &SourceSpan) -> Result<Place, String> {
    let ty = PlanType::List(ListLeaf::U8, 1);
    let output = temporary(builder, ty.clone(), span);
    let SourceConstant::Bytes(bytes) = value else {
        return Err("unsupported Body IR non-byte literal at bytes boundary".into());
    };
    list_call(
        builder,
        "empty_list",
        PlanType::U8,
        Vec::new(),
        ty,
        Vec::new(),
        output.clone(),
        span,
    );
    for byte in bytes {
        let reference_type = PlanType::ListMutRef(ListLeaf::U8, 1);
        let reference = temporary(builder, reference_type.clone(), span);
        builder.emit(plan_assign(
            reference.clone(),
            RvalueKind::MutBorrow(output.clone()),
            span,
        ));
        let argument = literal(Constant::Numeric(byte.to_string(), PlanType::U8), span);
        let unit = temporary(builder, PlanType::Unit, span);
        list_call(
            builder,
            "append_list",
            PlanType::U8,
            vec![reference_type, PlanType::U8],
            PlanType::Unit,
            vec![read_place(reference, true, span), argument],
            unit,
            span,
        );
    }
    Ok(output)
}
