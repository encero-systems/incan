//! Generic instance requests a keyed product carries restore the exact overload they were made for.

use incan_mir_plan_native::{decode_instance_request, encode_instance_request};
use incan_semantics_core::body_ir::{BodyIrModule, CallableParam, CallableParamDefault, LocalId};
use incan_semantics_core::{
    CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanCallableParamKind, IncanInputAccess,
    SemanticSourceTargetKind,
};

use super::bodies::empty_body;
use crate::instance_products::{request_identity, restore_requests};
use crate::type_parameters::Instance;

fn function_identity(name: &str, start: usize, end: usize) -> CanonicalSymbolId {
    CanonicalSymbolId::module_declaration(
        vec!["app".into()],
        name,
        SemanticSourceTargetKind::Function,
        HirSourceSpan::new(start, end),
    )
}

fn instance(body: incan_semantics_core::body_ir::Body) -> Instance {
    Instance {
        body,
        arguments: Vec::new(),
        owned_callable_slots: Vec::new(),
        original_callable: false,
    }
}

/// Reordering overloads preserves signed requests. An old unsigned request requires a unique match.
#[test]
fn generic_instance_requests_select_the_exact_overload() -> Result<(), String> {
    let mut one = empty_body("pick");
    one.canonical = Some(function_identity("pick", 0, 10));
    one.type_parameters = vec!["T".into()];
    let mut two = empty_body("pick");
    two.canonical = Some(function_identity("pick", 12, 28));
    two.span = HirSourceSpan::new(12, 28);
    two.direct_call_id = CompilerNodeId::declaration_span("app", 12, 28);
    two.type_parameters = vec!["T".into()];
    two.params = vec![CallableParam {
        local: LocalId(0),
        name: "value".into(),
        ty: one.return_type.clone(),
        kind: IncanCallableParamKind::Normal,
        span: one.span,
        default: CallableParamDefault::Required,
        mutable: false,
        native_owned: false,
        input_access: IncanInputAccess::Inferred,
    }];
    let mut module = BodyIrModule::declarations_only("app".into());
    module.bodies = vec![two.clone(), one.clone()];
    let short = request_identity(&instance(one.clone()))?;
    let long = request_identity(&instance(two.clone()))?;
    assert_ne!(
        short, long,
        "distinct checked signatures produced identical request bytes"
    );
    let Some(found) = restore_requests(&[short.clone(), long.clone()], &module) else {
        return Err("signed overload requests did not restore".into());
    };
    assert_eq!(found.len(), 2, "one of two restored overloads disappeared");
    assert!(
        found[0].body.params.is_empty(),
        "first signed request restored the wrong overload"
    );
    assert_eq!(
        found[1].body.params.len(),
        1,
        "second signed request restored the wrong overload"
    );
    let (stable, arguments, slots, original) = decode_instance_request(&short)?;
    let unsigned = stable.split('|').next().unwrap_or_default().to_owned();
    let old = encode_instance_request(&unsigned, &arguments, &slots, original)?;
    assert!(
        restore_requests(&[old.clone()], &module).is_none(),
        "an ambiguous legacy request was accepted"
    );
    module.bodies = vec![one];
    assert!(
        restore_requests(&[old], &module).is_some(),
        "a uniquely identified historical request was lost"
    );
    module.bodies = vec![two.clone(), two];
    assert!(
        restore_requests(&[long], &module).is_none(),
        "two current bodies had the same signed identity"
    );
    Ok(())
}
