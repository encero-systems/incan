//! A keyed product carries the closure frames its function's lowering lifted, and the calls into them.

use super::support::{assign_at, block, copy_at, empty_function, literal, local, place_at, returning};
use crate::body::{Callee, CalleeKind, Operand, Projection, RvalueKind, Terminator, TerminatorKind, Unwind};
use crate::plan::Function;
use crate::records::function::{decode_frames, decode_product, encode_product, product_frame_names, product_nominals};
use crate::types::{PlanType, SourceSpan, source_span};

/// A caller, its lifted closure frame and the closure-object calls between them round-trip as one product.
#[test]
fn product_carries_lifted_closure_frames() -> Result<(), String> {
    let span = source_span("closures.incn", 3, 1);
    let signature = vec![PlanType::Int, PlanType::Int];
    let item_span = source_span("closures.incn", 5, 9);
    let item = Callee {
        kind: CalleeKind::Planned("__incan_closure_apply_0".to_owned()),
        span: item_span,
    };
    let caller = Function::new(
        "apply".to_owned(),
        Vec::new(),
        PlanType::Int,
        vec![
            local(PlanType::Int, &span),
            local(PlanType::Closure(signature.clone()), &span),
            local(PlanType::ClosureRef(signature.clone()), &span),
            local(PlanType::Int, &span),
            local(
                PlanType::FunctionItem("__incan_closure_apply_0".to_owned(), signature.clone()),
                &span,
            ),
        ],
        vec![
            block(
                vec![
                    assign_at(4, Projection::Whole, RvalueKind::FunctionItem(item), &span),
                    assign_at(
                        2,
                        Projection::Whole,
                        RvalueKind::ClosureObject(place_at(1, Projection::Whole, &span)),
                        &span,
                    ),
                ],
                call(
                    CalleeKind::CallClosure(signature),
                    vec![copy_at(2, Projection::Whole, &span), literal(20, &span)],
                    3,
                    1,
                    &span,
                ),
                &span,
            ),
            block(
                Vec::new(),
                call(
                    CalleeKind::ClosureBody("__incan_closure_apply_0".to_owned(), 0),
                    vec![literal(1, &span)],
                    0,
                    2,
                    &span,
                ),
                &span,
            ),
            block(Vec::new(), returning(&span), &span),
        ],
        span.clone(),
    );
    let frame = empty_function("__incan_closure_apply_0", PlanType::Int, &span);
    let frames = vec![frame];
    let references = product_nominals(&caller, &[], &frames)?;
    assert!(references.is_empty());
    let product = encode_product(&caller, &[], &[], &frames)?;
    assert!(product_frame_names(&product)? == ["__incan_closure_apply_0"]);
    let (restored, _runtime) = decode_product(&product, &references)?;
    assert!(
        restored == caller,
        "the caller's closure calls did not survive its product"
    );
    let decoded = decode_frames(&product, &references)?;
    assert!(
        decoded == frames,
        "the lifted closure frame did not survive its product"
    );
    Ok(())
}

/// A call that continues to `target`.
fn call(callee: CalleeKind, arguments: Vec<Operand>, destination: i64, target: i64, span: &SourceSpan) -> Terminator {
    Terminator {
        kind: TerminatorKind::Call(
            Callee {
                kind: callee,
                span: span.clone(),
            },
            arguments,
            place_at(destination, Projection::Whole, span),
            target,
            Unwind::Continue,
        ),
        span: span.clone(),
    }
}
