//! Instantiate checked inherited instance bodies while preserving their original source identities.

use incan_semantics_core::body_ir::{Body, BodyIrModule, LocalOrigin, NominalDeclaration};
use incan_semantics_core::{CanonicalSymbolId, IncanType};

/// Add one concrete receiver frame per inherited implementation after original module validation.
pub fn specialize_classes(module: &BodyIrModule) -> Result<Vec<Body>, String> {
    let mut result = Vec::new();
    for declaration in &module.nominal_declarations {
        let Some(inheritance) = &declaration.class_inheritance else {
            continue;
        };
        let mut instantiated: Vec<CanonicalSymbolId> = Vec::new();
        for record in &inheritance.methods {
            if instantiated.contains(&record.implementation) {
                continue;
            }
            instantiated.push(record.implementation.clone());
            let mut found = false;
            for body in &module.bodies {
                if body.canonical.as_ref() != Some(&record.implementation) {
                    continue;
                }
                found = true;
                if body.locals.is_empty() {
                    return Err("unsupported Body IR inherited class receiver frame".into());
                }
                if body.locals[0].ty == IncanType::Named(declaration.name.clone()) {
                    continue;
                }
                result.push(instantiate_body(body, declaration)?);
            }
            if !found {
                return Err("unsupported Body IR inherited class implementation without source body".into());
            }
        }
    }
    Ok(result)
}

/// Close only the explicit receiver; other ancestor-typed self values remain a named unsupported boundary.
fn instantiate_body(body: &Body, owner: &NominalDeclaration) -> Result<Body, String> {
    if body.locals.is_empty() || body.params.is_empty() || !body.type_parameters.is_empty() || body.is_async {
        return Err("unsupported Body IR inherited class receiver frame".into());
    }
    if !matches!(body.locals[0].origin, LocalOrigin::Receiver { .. }) {
        return Err("unsupported Body IR inherited class method without instance receiver".into());
    }
    if body.locals[1..].iter().any(|local| local.ty == body.locals[0].ty) {
        return Err("unsupported Body IR inherited class self value beyond the receiver".into());
    }
    let mut result = body.clone();
    let receiver = IncanType::Named(owner.name.clone());
    result.locals[0].ty = receiver.clone();
    result.params[0].ty = receiver;
    Ok(result)
}
