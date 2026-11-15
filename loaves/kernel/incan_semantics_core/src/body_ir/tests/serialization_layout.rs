use super::*;
use crate::IncanPrimitiveType;
use serde::Serialize;

#[allow(dead_code)]
#[derive(Serialize)]
enum LegacyRvalue {
    Use(Operand),
    UnaryOp(UnOp, Operand),
    BinaryOp(BinOp, Operand, Operand),
    IsInstance {
        value: Operand,
        value_ty: IncanType,
        target: IsInstanceTarget,
    },
}

#[derive(Serialize)]
struct LegacyLocalDecl {
    id: LocalId,
    name: Option<String>,
    identity: Option<CanonicalSymbolId>,
    ty: IncanType,
    origin: LocalOrigin,
    scope: ScopeId,
    span: HirSourceSpan,
}

#[test]
fn boxed_isinstance_payload_preserves_postcard_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let identity = CanonicalSymbolId::module_declaration(
        vec!["app".to_string()],
        "Value",
        SemanticSourceTargetKind::Class,
        HirSourceSpan::new(10, 20),
    );
    let value = Operand::place(Place::from_local(LocalId(3)), OwnershipFact::Borrow, false);
    let value_ty = IncanType::Primitive(IncanPrimitiveType::Int);
    let target = IsInstanceTarget {
        ty: IncanType::Primitive(IncanPrimitiveType::Int),
        canonical: Some(identity),
        span: HirSourceSpan::new(30, 35),
    };
    let legacy = LegacyRvalue::IsInstance {
        value: value.clone(),
        value_ty: value_ty.clone(),
        target: target.clone(),
    };
    let current = Rvalue::IsInstance(Box::new(IsInstanceRvalue {
        value,
        value_ty,
        target,
    }));

    assert_eq!(postcard::to_allocvec(&legacy)?, postcard::to_allocvec(&current)?);
    Ok(())
}

#[test]
fn boxed_local_identity_preserves_postcard_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let identity = CanonicalSymbolId::module_declaration(
        vec!["app".to_string()],
        "value",
        SemanticSourceTargetKind::Local,
        HirSourceSpan::new(10, 15),
    );
    let legacy = LegacyLocalDecl {
        id: LocalId(2),
        name: Some("value".to_string()),
        identity: Some(identity.clone()),
        ty: IncanType::Primitive(IncanPrimitiveType::Int),
        origin: LocalOrigin::UserBinding,
        scope: ScopeId(1),
        span: HirSourceSpan::new(10, 15),
    };
    let current = LocalDecl {
        id: LocalId(2),
        name: Some("value".to_string()),
        identity: Some(Box::new(identity)),
        ty: IncanType::Primitive(IncanPrimitiveType::Int),
        origin: LocalOrigin::UserBinding,
        scope: ScopeId(1),
        span: HirSourceSpan::new(10, 15),
    };

    assert_eq!(postcard::to_allocvec(&legacy)?, postcard::to_allocvec(&current)?);
    Ok(())
}
