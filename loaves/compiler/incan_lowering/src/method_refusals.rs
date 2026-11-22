//! The refusal text of a method call whose selected implementation has no matching receiver frame.

use incan_mir_plan_native::PlanType;
use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::body_ir::{BodyIrModule, MethodTarget};

use crate::classes::frame_type;
use crate::frame::Builder;

/// How many candidate bodies the refusal describes; it still counts every one.
const CANDIDATES_LIMIT: usize = 6;

/// Describe a selection no candidate body satisfied: the target, the receiver owner and its identity, the selected
/// implementation, and up to six bodies carrying that implementation's identity with their receiver frames.
///
/// Only the refusal path computes this; no candidate or receiver check of the selection changes.
pub fn owner_method_refusal(
    module: &BodyIrModule,
    builder: &Builder,
    target: &MethodTarget,
    owner: &PlanType,
    identity: &CanonicalSymbolId,
    implementation: &CanonicalSymbolId,
    candidates: &[usize],
) -> String {
    let canonical = Some(implementation.clone());
    let mut count = 0;
    let mut described = String::new();
    for index in candidates {
        let body = &module.bodies[*index];
        if body.canonical != canonical {
            continue;
        }
        count += 1;
        if count > CANDIDATES_LIMIT {
            continue;
        }
        let parameters = body.params.len();
        let Some(first) = body.locals.first() else {
            described += &format!("{}(params={parameters}, receiver=<no locals>); ", body.name);
            continue;
        };
        described += &match frame_type(
            builder.constructors(),
            builder.nominals(),
            builder.enums(),
            first,
            &builder.intrinsics,
            "",
        ) {
            Ok(receiver) => format!(
                "{}(params={parameters}, raw_receiver={}, frame={receiver:?}); ",
                body.name, first.ty
            ),
            Err(error) => format!(
                "{}(params={parameters}, raw_receiver={}, frame_error={error}); ",
                body.name, first.ty
            ),
        };
    }
    format!(
        "unsupported Body IR nonlocal or trait-object Method; target={:?}; owner={owner:?}; owner_identity={identity:?}; implementation={canonical:?}; canonical_candidates={count}; candidates_limit={CANDIDATES_LIMIT}; candidates=[{described}]",
        target.canonical
    )
}
