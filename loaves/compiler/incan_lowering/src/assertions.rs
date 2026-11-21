//! Lower checked condition and carrier-pattern assertions through the legacy assertion runtime contracts.

use incan_lang::errors::ErrorKind;
use incan_lang::lang::errors::as_str as runtime_error_name;
use incan_mir_plan_native::{Carrier, PlanType, SourceSpan, TerminatorKind, assign as plan_assign};
use incan_semantics_core::body_ir::{AssertionKind, Constant as SourceConstant, Operand as SourceOperand, Pattern};

use crate::checked_values::{scalar_operand, scalar_place};
use crate::closures::function_assignment;
use crate::formatting::display_operand;
use crate::frame::{Builder, external_call, place_type, read_place, temporary, value_type};
use crate::matches::bind_enum_payload;
use crate::options::{carrier_layout, intrinsic_pattern, variant_condition};
use crate::patterns::has_refutable_payload;
use crate::text_plan::owned_literal;

/// Branch before materializing failure text, or invoke a retained unit callable inside the unwind boundary.
pub fn lower_assertion(
    builder: &mut Builder,
    kind: &AssertionKind,
    message: Option<&SourceOperand>,
    span: &SourceSpan,
) -> Result<(), String> {
    let operand = match kind {
        AssertionKind::Condition { cond } => scalar_operand(builder, cond, span)?,
        AssertionKind::Pattern { scrutinee, pattern } => {
            return lower_pattern_assertion(builder, scrutinee, pattern, message, span);
        }
        AssertionKind::Raises { call, expected_error } => {
            return lower_raises_assertion(builder, call, *expected_error, message, span);
        }
    };
    // Body IR's lazy-message failure block ends in `assert false`; it has no success edge or owner merge.
    if let AssertionKind::Condition {
        cond: SourceOperand::Constant(SourceConstant::Bool(flag)),
    } = kind
    {
        if *flag {
            return Ok(());
        }
        return lower_failure(builder, message, span);
    }
    let failed = builder.new_block(span);
    let passed = builder.new_block(span);
    let incoming = builder.owned.clone();
    builder.finish(TerminatorKind::SwitchBool(operand, failed, passed), span);
    builder.select(failed);
    lower_failure(builder, message, span)?;
    builder.select(passed);
    builder.owned = incoming;
    Ok(())
}

/// Transfer the failure payload to the unwinding runtime and close the unreachable normal continuation.
fn lower_failure(builder: &mut Builder, message: Option<&SourceOperand>, span: &SourceSpan) -> Result<(), String> {
    // The text temporary is reserved before the message selects its own storage, as the reference lowering reserves it.
    temporary(builder, PlanType::String, span);
    let text = match message {
        Some(value) => display_operand(builder, value, span)?,
        None => owned_literal(builder, "", span),
    };
    let destination = temporary(builder, PlanType::Unit, span);
    external_call(
        builder,
        "incan_native_runtime::assert_failure",
        vec![PlanType::String],
        PlanType::Unit,
        vec![read_place(text, true, span)],
        destination,
        span,
    );
    builder.finish(TerminatorKind::Unreachable, span);
    Ok(())
}

/// Test one `Some`, `None`, `Ok` or `Err` carrier pattern as legacy's inlined std.testing helper does.
///
/// Legacy evaluates an explicit message only on the failing path, while Body IR evaluates it before the assertion, so
/// only a literal message, which has no effects to reorder, is admitted. A mismatch fails through the same std.testing
/// helper; a match binds the payload for later statements.
fn lower_pattern_assertion(
    builder: &mut Builder,
    scrutinee: &SourceOperand,
    pattern: &Pattern,
    message: Option<&SourceOperand>,
    span: &SourceSpan,
) -> Result<(), String> {
    let SourceOperand::Place(read) = scrutinee else {
        return Err("unsupported Body IR nonplace pattern Assert scrutinee".into());
    };
    let source = scalar_place(builder, &read.place, span)?;
    let text = match message {
        Some(SourceOperand::Constant(SourceConstant::Str(value))) => Some(value.clone()),
        Some(_) => return Err("unsupported Body IR computed pattern Assert message".into()),
        None => None,
    };
    let (_, declaration) = carrier_layout(builder, &place_type(&builder.locals, &source))?;
    // A literal or nested payload would need its own tests; the assertion compares only the variant.
    if has_refutable_payload(pattern) {
        return Err("unsupported Body IR nested pattern Assert".into());
    }
    let slot = intrinsic_pattern(&declaration, pattern)?;
    if slot < 0 {
        return Err("unsupported Body IR wildcard pattern Assert".into());
    }
    let condition = variant_condition(builder, &source, slot, span);
    let failed = builder.new_block(span);
    let passed = builder.new_block(span);
    let incoming = builder.owned.clone();
    builder.finish(TerminatorKind::SwitchBool(condition, failed, passed), span);
    builder.select(failed);
    let failure = format!("incan_native_runtime::{}", pattern_failure(&declaration.carrier, slot));
    let destination = temporary(builder, PlanType::Unit, span);
    match text {
        Some(text) => {
            let payload = read_place(owned_literal(builder, &text, span), true, span);
            let path = format!("{failure}_with");
            external_call(
                builder,
                &path,
                vec![PlanType::String],
                PlanType::Unit,
                vec![payload],
                destination,
                span,
            );
        }
        None => external_call(
            builder,
            &failure,
            Vec::new(),
            PlanType::Unit,
            Vec::new(),
            destination,
            span,
        ),
    }
    builder.finish(TerminatorKind::Unreachable, span);
    builder.select(passed);
    builder.owned = incoming;
    bind_enum_payload(builder, &source, slot, pattern, span)
}

/// Name the runtime failure for a carrier pattern; Option's slot one is Some and Result's slot zero is Ok.
fn pattern_failure(carrier: &Carrier, slot: i64) -> &'static str {
    if *carrier == Carrier::Option {
        return if slot == 1 { "missing_some" } else { "unexpected_some" };
    }
    if slot == 0 { "missing_ok" } else { "missing_err" }
}

/// Reify a checked zero-argument unit function and retain only effect-free literal messages.
fn lower_raises_assertion(
    builder: &mut Builder,
    call: &SourceOperand,
    expected: ErrorKind,
    message: Option<&SourceOperand>,
    span: &SourceSpan,
) -> Result<(), String> {
    let text = match message {
        Some(SourceOperand::Constant(SourceConstant::Str(value))) => value.clone(),
        Some(_) => return Err("unsupported Body IR computed raises Assert message".into()),
        None => String::new(),
    };
    let operand = scalar_operand(builder, call, span)?;
    let pointer_type = PlanType::FunctionPointer(vec![PlanType::Unit]);
    let source_type = value_type(builder, &operand)?;
    match &source_type {
        PlanType::FunctionItem(_, signature) | PlanType::FunctionPointer(signature) => {
            if *signature != [PlanType::Unit] {
                return Err("unsupported Body IR raises Assert callable outside zero-argument unit functions".into());
            }
        }
        _ => return Err("unsupported Body IR raises Assert without a deferred function".into()),
    }
    let context = format!("raises Assert callable in {}", builder.name);
    let (ty, operation) = function_assignment(&pointer_type, &source_type, operand, true, &context)?;
    let pointer = temporary(builder, ty, span);
    builder.emit(plan_assign(pointer.clone(), operation, span));
    let error_name = owned_literal(builder, runtime_error_name(expected), span);
    let failure_message = owned_literal(builder, &text, span);
    let destination = temporary(builder, PlanType::Unit, span);
    external_call(
        builder,
        "incan_native_runtime::assert_raises_deferred",
        vec![pointer_type, PlanType::String, PlanType::String],
        PlanType::Unit,
        vec![
            read_place(pointer, false, span),
            read_place(error_name, true, span),
            read_place(failure_message, true, span),
        ],
        destination,
        span,
    );
    Ok(())
}
