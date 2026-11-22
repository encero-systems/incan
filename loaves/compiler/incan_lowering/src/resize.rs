//! Checked builtin numeric resize calls reuse legacy casts and core policy functions.

use incan_lang::lang::types::numerics::NumericResizeMethodId;
use incan_mir_plan_native::{
    CalleeKind, Carrier, ExternalFunction, Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign,
};

use crate::frame::{Builder, place_type, terminate_call};
use crate::numerics::numeric_carrier;
use crate::options::carrier_layout;

/// Use the checked output carrier, including the retained Some payload for checked resizing.
pub fn emit_resize(
    builder: &mut Builder,
    policy: NumericResizeMethodId,
    source: PlanType,
    operand: Operand,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let result = place_type(&builder.locals, &output);
    let mut target = result.clone();
    if policy == NumericResizeMethodId::Try {
        let (_, declaration) = carrier_layout(builder, &result)?;
        if declaration.carrier != Carrier::Option || declaration.variants.len() != 2 {
            return Err("invalid Body IR checked resize carrier".into());
        }
        // An Option carrier lays out None then Some, so the payload is the second variant's.
        let payloads = &declaration.variants[1].fields;
        if payloads.len() != 1 {
            return Err("invalid Body IR checked resize payload".into());
        }
        target = payloads[0].clone();
    }
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
        NumericResizeMethodId::Try => "try_resize",
        NumericResizeMethodId::Saturating => "saturating_resize",
        _ => return Err("unsupported Body IR numeric resize policy".into()),
    };
    let path = format!("incan_std_primitives::num::{policy_name}");
    builder.externals.push(ExternalFunction {
        path: path.clone(),
        type_arguments: vec![source.clone(), target.clone()],
        parameters: vec![source.clone()],
        return_type: result,
        span: span.clone(),
    });
    terminate_call(
        builder,
        CalleeKind::Instantiated(path, vec![source, target]),
        vec![operand],
        output,
        span,
    );
    Ok(())
}
