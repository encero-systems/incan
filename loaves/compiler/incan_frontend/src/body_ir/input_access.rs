//! Canonical source input requests, independent of binding mutation and native collection ownership. Transporting
//! these facts alone does not admit execution; the owning body must retain its independently checked input profile.

use incan_semantics_core::{CanonicalSymbolId, IncanInputAccess, IncanType};

use super::{TypeCheckInfo, bir};

/// Apply a direct checked request to its source parameter slot. Nested and returned-reference input dependencies
/// remain unsupported types; missing legacy metadata grants only ordinary inference.
pub(super) fn apply(
    parameter: &mut bir::CallableParam,
    facts: &TypeCheckInfo,
    owner: Option<&CanonicalSymbolId>,
    slot: usize,
) {
    match selected(facts, owner, slot) {
        Ok(access) if !parameter.input_access.is_inferred() && parameter.input_access != access => {
            parameter.ty = IncanType::Unsupported("callable access differs from its canonical input contract".into());
        }
        Ok(access) => parameter.input_access = access,
        Err(reason) => parameter.ty = IncanType::Unsupported(reason.into()),
    }
}

/// Resolve the canonical declaration and structural slot, never the spelling of its type or parameter.
pub(super) fn selected(
    facts: &TypeCheckInfo,
    owner: Option<&CanonicalSymbolId>,
    slot: usize,
) -> Result<IncanInputAccess, &'static str> {
    crate::ownership_metadata::direct_input_access(
        owner.and_then(|owner| facts.declarations.ownership_contracts.get(owner)),
        slot,
    )
}

/// Select the checked value before allocating its local, so callable and local types agree. Invalid canonical
/// metadata becomes an explicit unsupported type and cannot fall back to a wrapper's incidental layout.
pub(super) fn value_type(
    builder: &super::BodyBuilder<'_, '_>,
    owner: Option<&CanonicalSymbolId>,
    slot: usize,
    parameter: &crate::symbols::ResolvedType,
) -> IncanType {
    let contract = owner.and_then(|owner| builder.type_info.declarations.ownership_contracts.get(owner));
    match crate::ownership_metadata::input_value_type(contract, slot, parameter) {
        Ok(value) => builder.checked_type(value),
        Err(reason) => IncanType::Unsupported(reason.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{CanonicalSymbolId, IncanInputAccess, IncanType, TypeCheckInfo, apply, bir, selected};
    use crate::ownership_metadata::{AccessMode, OwnershipContract, TypeComponent};
    use crate::symbols::ResolvedType;
    use incan_lang::lang::types::ownership::OwnershipMode;
    use incan_semantics_core::executable_representation::{
        EXECUTABLE_REPRESENTATION_VERSION, require_supported_version,
    };
    use incan_semantics_core::semantic_digest::semantic_digest;
    use incan_semantics_core::stable_identity::source_signature::SourceSignatureContext;
    use incan_semantics_core::{
        HirSourceSpan, IncanCallableParam, IncanCallableParamKind, IncanPrimitiveType, SemanticSourceTargetKind,
    };
    use std::collections::{BTreeMap, BTreeSet};

    /// Independent mutation and native ABI flags make accidental conflation observable.
    fn parameter() -> bir::CallableParam {
        bir::CallableParam {
            local: bir::LocalId(0),
            name: "value".into(),
            ty: IncanType::Primitive(IncanPrimitiveType::Int),
            kind: IncanCallableParamKind::Normal,
            span: HirSourceSpan::new(4, 9),
            default: bir::CallableParamDefault::Required,
            mutable: true,
            native_owned: true,
            input_access: IncanInputAccess::Inferred,
        }
    }

    /// Canonical source slots retain access while nested requests refuse and missing metadata remains inferred.
    #[test]
    fn canonical_inputs_preserve_independent_facts() -> Result<(), Box<dyn std::error::Error>> {
        let owner = CanonicalSymbolId::module_declaration(
            vec!["main".into()],
            "take",
            SemanticSourceTargetKind::Function,
            HirSourceSpan::new(0, 30),
        );
        let mut facts = TypeCheckInfo::default();
        for (mode, expected) in [
            (OwnershipMode::Borrowed, IncanInputAccess::Shared),
            (OwnershipMode::BorrowedMut, IncanInputAccess::Exclusive),
            (OwnershipMode::Owned, IncanInputAccess::Owned),
        ] {
            facts.declarations.ownership_contracts.insert(
                owner.clone(),
                OwnershipContract {
                    schema_version: 1,
                    modes: vec![AccessMode {
                        mode,
                        component: TypeComponent {
                            parameter: Some(1),
                            children: Vec::new(),
                        },
                        underlying: ResolvedType::Int,
                    }],
                    result_source: None,
                },
            );
            let mut projected = parameter();
            apply(&mut projected, &facts, Some(&owner), 1);
            assert_eq!(projected.input_access, expected);
            assert!(projected.mutable && projected.native_owned);
            assert_eq!(selected(&facts, Some(&owner), 0)?, IncanInputAccess::Inferred);
            let contract = facts
                .declarations
                .ownership_contracts
                .get_mut(&owner)
                .ok_or("missing contract")?;
            contract.modes[0].component.children.push(0);
            apply(&mut projected, &facts, Some(&owner), 1);
            assert!(matches!(projected.ty, IncanType::Unsupported(_)));
        }
        let contract = facts
            .declarations
            .ownership_contracts
            .get_mut(&owner)
            .ok_or("missing contract")?;
        contract.schema_version = 2;
        assert!(selected(&facts, Some(&owner), 1).is_err());
        assert_eq!(selected(&facts, None, 1)?, IncanInputAccess::Inferred);
        let mut orphaned = parameter();
        orphaned.input_access = IncanInputAccess::Shared;
        apply(&mut orphaned, &facts, None, 1);
        assert_eq!(orphaned.input_access, IncanInputAccess::Shared);
        assert!(matches!(orphaned.ty, IncanType::Unsupported(_)));
        let mut missing = owner;
        missing.declaration_name = "other".into();
        assert_eq!(selected(&facts, Some(&missing), 1)?, IncanInputAccess::Inferred);
        Ok(())
    }

    /// The same tag survives a cloned frame, a partial preset and a closure retained inside a deferred default.
    #[test]
    fn access_survives_callable_containers() -> Result<(), Box<dyn std::error::Error>> {
        for access in [
            IncanInputAccess::Inferred,
            IncanInputAccess::Shared,
            IncanInputAccess::Exclusive,
            IncanInputAccess::Owned,
        ] {
            let mut direct = parameter();
            direct.input_access = access;
            let mut partial = direct.clone();
            partial.default = bir::CallableParamDefault::PartialPreset {
                capture: bir::LocalId(2),
            };
            let closure = bir::Rvalue::Closure {
                params: vec![direct.clone(), partial],
                captured_operands: Vec::new(),
                body: Box::new(bir::ClosureBody {
                    native_storage: None,
                    original_callable: None,
                    function_item: false,
                    consumes_environment: false,
                    capture_locals: Vec::new(),
                    stmts: Vec::new(),
                    result: bir::Operand::Constant(bir::Constant::Unit),
                }),
            };
            let wrapper = bir::CallableParamDefault::Source(Box::new(bir::DefaultComputation {
                span: HirSourceSpan::new(30, 40),
                stmts: vec![bir::Statement {
                    kind: bir::StatementKind::Assign {
                        place: bir::Place::from_local(bir::LocalId(3)),
                        rvalue: closure,
                    },
                    span: HirSourceSpan::new(31, 39),
                }],
                result: bir::Operand::Constant(bir::Constant::Unit),
            }));
            let decoded: bir::CallableParamDefault = postcard::from_bytes(&postcard::to_allocvec(&wrapper)?)?;
            assert_eq!(decoded, wrapper);
            let mut named = serde_json::to_value(&direct)?;
            named.as_object_mut().ok_or("not an object")?.remove("input_access");
            let legacy: bir::CallableParam = serde_json::from_value(named)?;
            assert_eq!(legacy.input_access, IncanInputAccess::Inferred);
        }
        for obsolete in [35_u32, 36] {
            assert!(require_supported_version(&postcard::to_allocvec(&obsolete)?).is_err());
        }
        assert!(require_supported_version(&postcard::to_allocvec(&EXECUTABLE_REPRESENTATION_VERSION)?).is_ok());
        Ok(())
    }

    /// Input access changes both source and executable identity without changing the underlying type.
    #[test]
    fn access_participates_in_semantic_identity() -> Result<(), Box<dyn std::error::Error>> {
        let mut source_keys = BTreeSet::new();
        let mut body_keys = BTreeSet::new();
        let nominals = BTreeMap::new();
        let context = SourceSignatureContext {
            nominals: &nominals,
            binders: &[],
        };
        for input_access in [
            IncanInputAccess::Inferred,
            IncanInputAccess::Shared,
            IncanInputAccess::Exclusive,
            IncanInputAccess::Owned,
        ] {
            let callable = IncanCallableParam {
                name: Some("value".into()),
                ty: IncanType::Primitive(IncanPrimitiveType::Int),
                kind: IncanCallableParamKind::Normal,
                has_default: false,
                is_partial_preset: false,
                is_mut: false,
                native_owned: false,
                input_access,
            };
            let signature = context
                .callable(&[callable], &IncanType::Primitive(IncanPrimitiveType::Unit), false)
                .ok_or("missing signature")?;
            assert!(signature.as_str().starts_with("incan.source-signature/2:"));
            source_keys.insert(signature.as_str().to_owned());
            let mut executable = parameter();
            executable.input_access = input_access;
            body_keys.insert(semantic_digest(&executable)?);
        }
        assert_eq!(source_keys.len(), 4);
        assert_eq!(body_keys.len(), 4);
        Ok(())
    }
    /// Source refusal stays active while the staged lowering inspection proves frame locals use the payload type.
    #[test]
    fn source_parameters_and_locals_share_projected_value_types() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def read(value: Borrowed[int]) -> int:\n    return value\ndef take(value: Owned[int]) -> int:\n    return value\nclass Reader:\n    def read(self, value: Borrowed[int]) -> int:\n        return value\n";
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = crate::typechecker::TypeChecker::new();
        let errors = checker
            .check_program(&program)
            .err()
            .ok_or("explicit execution gate unexpectedly opened")?;
        assert!(
            errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{errors:?}"
        );
        let facts = checker.into_type_info();
        let providers = super::super::ProviderOperationCatalog::new();
        let context = super::super::BodyIrContext {
            modules: &[],
            provider_operations: &providers,
            registry_package: None,
            library_package: None,
        };
        let module = super::super::build_module_seed(&program, &[], &facts, &context);
        let mut inputs = 0;
        for body in &module.bodies {
            for parameter in &body.params {
                if parameter.input_access.is_inferred() {
                    continue;
                }
                inputs += 1;
                assert_eq!(parameter.ty, IncanType::Primitive(IncanPrimitiveType::Int));
                let local = body
                    .locals
                    .get(parameter.local.index())
                    .ok_or("missing parameter local")?;
                assert_eq!(local.ty, parameter.ty);
            }
        }
        assert_eq!(inputs, 3);
        Ok(())
    }

    /// Checking a source alias does not authorize lowering it as an ordinary owned copy or first-class reference.
    #[test]
    fn local_source_alias_waits_for_storage_projection() -> Result<(), Box<dyn std::error::Error>> {
        let source =
            "def read(value: Borrowed[str]) -> int:\n    let view: Borrowed[str] = value\n    return len(view)\n";
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = crate::typechecker::TypeChecker::new();
        let errors = checker
            .check_program(&program)
            .err()
            .ok_or("source execution gate unexpectedly opened")?;
        assert!(
            errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{errors:?}"
        );
        let facts = checker.into_type_info();
        let providers = super::super::ProviderOperationCatalog::new();
        let context = super::super::BodyIrContext {
            modules: &[],
            provider_operations: &providers,
            registry_package: None,
            library_package: None,
        };
        let module = super::super::build_module_seed(&program, &[], &facts, &context);
        let body = module.bodies.first().ok_or("missing body")?;
        assert!(body.block.stmts.iter().any(|statement| matches!(&statement.kind,
            bir::StatementKind::Unsupported { description } if description.contains("non-Copy local alias"))));
        assert!(
            !body
                .locals
                .iter()
                .any(|local| local.origin == bir::LocalOrigin::UserBinding && local.name.as_deref() == Some("view"))
        );
        Ok(())
    }
}
