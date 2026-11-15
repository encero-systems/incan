//! Preserve accepted scoped-surface ownership and syntax in the public worker request.

use serde_json::{Value, json};

use crate::ast;

use super::expression;

/// Translate a descriptor-owned glyph or leading-dot path without assigning it a builtin operation.
/// Other compiler surfaces and scoped symbol calls require their own public syntax contracts.
pub(super) fn encode(surface: &ast::SurfaceExpr) -> Result<Value, String> {
    let incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
        dependency_key,
        descriptor_key,
    } = &surface.key
    else {
        return Err("host surface is not represented by the public vocabulary schema".into());
    };
    let (kind, owner) = match &surface.payload {
        ast::SurfaceExprPayload::LeadingDotPath {
            segments,
            receiver,
            owner,
        } => (json!({"LeadingDotPath": [segments, receiver_value(receiver)?]}), owner),
        ast::SurfaceExprPayload::ScopedGlyph {
            glyph,
            left,
            right,
            owner,
        } => (
            json!({"Glyph": [glyph, [expression(left)?, expression(right)?]]}),
            owner,
        ),
        _ => return Err("scoped surface payload is not represented by the public vocabulary schema".into()),
    };
    Ok(
        json!({"descriptor_key": descriptor_key, "dependency_key": dependency_key, "kind": kind,
        "owner": {"declaration": owner.declaration, "clause": owner.clause, "call": owner.call}}),
    )
}

/// Encode the public worker's tuple variants separately from descriptor metadata's named-field variants.
fn receiver_value(receiver: &incan_vocab::ScopedSurfaceReceiver) -> Result<Value, String> {
    let value = match receiver {
        incan_vocab::ScopedSurfaceReceiver::OwningDeclaration => json!("OwningDeclaration"),
        incan_vocab::ScopedSurfaceReceiver::Clause { clause } => json!({"Clause": clause}),
        incan_vocab::ScopedSurfaceReceiver::Custom { key } => json!({"Custom": key}),
        _ => return Err("scoped receiver is not represented by the public vocabulary schema".into()),
    };
    Ok(value)
}
