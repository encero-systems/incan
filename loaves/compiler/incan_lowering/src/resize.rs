//! Checked builtin numeric resize calls reuse legacy casts and core policy functions.

use incan_lang::lang::types::numerics::NumericResizeMethodId;
use incan_mir_plan_native::{
    CalleeKind, ExternalFunction, Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign,
};

use crate::frame::{Builder, place_type, terminate_call};
use crate::numerics::numeric_carrier;
use crate::pending::untranslated;

/// Use the checked output carrier, including the retained Some payload for checked resizing.
///
/// A checked resize reads its Option carrier through `options::carrier_layout`, which refuses by name until it is in
/// Rust.
pub fn emit_resize(
    builder: &mut Builder,
    policy: NumericResizeMethodId,
    source: PlanType,
    operand: Operand,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let result = place_type(&builder.locals, &output);
    if policy == NumericResizeMethodId::Try {
        return Err(untranslated("options::carrier_layout"));
    }
    let target = result.clone();
    if !numeric_carrier(&target) {
        return Err("unsupported Body IR resize destination outside numeric carriers".into());
    }
    if matches!(
        policy,
        NumericResizeMethodId::Lossless | NumericResizeMethodId::Wrapping
    ) {
        builder.emit(plan_assign(
            output,
            RvalueKind::NumericCast(operand, source, target),
            span,
        ));
        return Ok(());
    }
    let policy_name = match policy {
        NumericResizeMethodId::Saturating => "saturating_resize",
        _ => return Err("unsupported Body IR numeric resize policy".into()),
    };
    let path = format!("incan_std_primitives::num::{policy_name}");
    let types = vec![source.clone(), target];
    builder.externals.push(ExternalFunction {
        path: path.clone(),
        type_arguments: types.clone(),
        parameters: vec![source],
        return_type: result,
        span: span.clone(),
    });
    terminate_call(
        builder,
        CalleeKind::Instantiated(path, types),
        vec![operand],
        output,
        span,
    );
    Ok(())
}
