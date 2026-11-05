//! Tests for Body IR identities, input profiles, snapshot rendering and provider operation plans.

use super::*;
use crate::{CompilerNodeKind, IncanPrimitiveType, SemanticSourceTargetKind};

mod identities_and_profiles;
mod matches_and_providers;
mod rendering;

/// Build a scalar-only body for structural and serialization tests, without callable representation facts.
fn sample_body() -> Body {
    let decl_id = CompilerNodeId::declaration("m", "add");
    let direct_call_id = CompilerNodeId::declaration_span("m", 0, 30);
    let local_x = LocalId(0);
    let local_y = LocalId(1);
    let local_tmp = LocalId(2);
    Body {
        nominal_instantiation: Vec::new(),
        trait_default_instantiation: None,
        type_parameters: Vec::new(),
        decl_id: decl_id.clone(),
        direct_call_id,
        canonical: None,
        name: "add".to_string(),
        span: HirSourceSpan::new(0, 30),
        return_type: IncanType::Primitive(IncanPrimitiveType::Int),
        checked_input_profile: None,
        explicit_input_bindings: Vec::new(),
        returned_view: None,
        callable_representation: None,
        named_type_identities: Default::default(),
        locals: vec![
            LocalDecl {
                id: local_x,
                name: Some("x".to_string()),
                identity: None,
                ty: IncanType::Primitive(IncanPrimitiveType::Int),
                origin: LocalOrigin::Parameter,
                scope: ScopeId(0),
                span: HirSourceSpan::new(4, 5),
            },
            LocalDecl {
                id: local_y,
                name: Some("y".to_string()),
                identity: None,
                ty: IncanType::Primitive(IncanPrimitiveType::Int),
                origin: LocalOrigin::Parameter,
                scope: ScopeId(0),
                span: HirSourceSpan::new(7, 8),
            },
            LocalDecl {
                id: local_tmp,
                name: None,
                identity: None,
                ty: IncanType::Primitive(IncanPrimitiveType::Int),
                origin: LocalOrigin::Temporary,
                scope: ScopeId(0),
                span: HirSourceSpan::new(20, 25),
            },
        ],
        params: vec![
            CallableParam {
                mutable: false,
                input_access: crate::IncanInputAccess::Inferred,
                native_owned: false,
                kind: IncanCallableParamKind::Normal,
                local: local_x,
                name: "x".to_string(),
                ty: IncanType::Primitive(IncanPrimitiveType::Int),
                span: HirSourceSpan::new(4, 5),
                default: CallableParamDefault::Required,
            },
            CallableParam {
                mutable: false,
                input_access: crate::IncanInputAccess::Inferred,
                native_owned: false,
                kind: IncanCallableParamKind::Normal,
                local: local_y,
                name: "y".to_string(),
                ty: IncanType::Primitive(IncanPrimitiveType::Int),
                span: HirSourceSpan::new(7, 8),
                default: CallableParamDefault::Required,
            },
        ],
        param_locals: vec![local_x, local_y],
        scopes: vec![ScopeInfo {
            id: ScopeId(0),
            parent: None,
            span: HirSourceSpan::new(0, 30),
        }],
        block: Block {
            scope: ScopeId(0),
            stmts: vec![
                Statement {
                    kind: StatementKind::Assign {
                        place: Place::from_local(local_tmp),
                        rvalue: Rvalue::BinaryOp(
                            BinOp::Add,
                            Operand::place(Place::from_local(local_x), OwnershipFact::Copy, false),
                            Operand::place(Place::from_local(local_y), OwnershipFact::Copy, true),
                        ),
                    },
                    span: HirSourceSpan::new(20, 25),
                },
                Statement {
                    kind: StatementKind::Return {
                        value: Some(Operand::place(Place::from_local(local_tmp), OwnershipFact::Move, true)),
                    },
                    span: HirSourceSpan::new(20, 30),
                },
            ],
        },
        runtime_requirements: Vec::new(),
        panic_facts: Vec::new(),
        is_async: false,
        extern_delegation: None,
        native_attributes: Vec::new(),
    }
}

fn sample_global_place(kind: SemanticSourceTargetKind, write_policy: GlobalWritePolicy) -> Place {
    Place::from_global(GlobalPlace {
        identity: CanonicalSymbolId::module_declaration(
            vec!["app".to_string()],
            "VALUE",
            kind,
            HirSourceSpan::new(0, 5),
        ),
        ty: IncanType::Primitive(IncanPrimitiveType::Int),
        write_policy,
    })
}

/// Build an admitted plan for a `pub::billing.charge` operation requiring `host.http.request`.
fn sample_provider_operation_plan() -> ProviderOperationPlan {
    ProviderOperationPlan {
        operation: CanonicalSymbolId {
            namespace: crate::SymbolNamespace::OrdinaryLexical,
            origin: crate::SymbolOrigin::Package {
                library: "billing".to_string(),
                module_path: vec!["api".to_string()],
            },
            declaration_name: "charge".to_string(),
            kind: SemanticSourceTargetKind::Function,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(40, 60),
        },
        provider: ProviderActivation {
            provider_key: "billing@1.2.3#abc[]".to_string(),
            module_path: vec!["billing".to_string(), "api".to_string()],
            state: ProviderActivationState::Active,
        },
        required_capability: CanonicalSymbolId::module_declaration(
            vec!["host".to_string(), "http".to_string()],
            "request",
            SemanticSourceTargetKind::Capability,
            HirSourceSpan::new(10, 20),
        ),
        runtime_requirements: vec![AbiV0RuntimeRequirement::HostedStd],
        inputs: vec![ProviderOperationInput {
            slot: 0,
            written_position: 0,
            ty: IncanType::Primitive(IncanPrimitiveType::Str),
            span: HirSourceSpan::new(120, 128),
        }],
        call_span: HirSourceSpan::new(110, 130),
    }
}
