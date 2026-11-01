//! Publish compiler-owned source signatures after collection, independently of executable representations.

use std::collections::{BTreeMap, HashMap};

use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::stable_identity::source_signature::SourceSignatureContext;
use incan_semantics_core::stable_identity::{StableDeclarationContext, StableDeclarationId};

use crate::ast::{Declaration, MethodDecl, Program, Receiver, Spanned, TypeParam};
use crate::ownership_metadata::{CheckedOwnershipContract, source_input_access};
use crate::symbols::{CallableParam, MethodInfo, ResolvedType, SymbolKind, TypeInfo};

use super::type_info::semantic_callable_param_from_resolved;
use super::{TypeChecker, semantic_type_from_resolved};

impl TypeChecker {
    /// Freeze source identities using settled collected signatures, before defaults, bodies or decorator rebinding.
    pub(super) fn record_source_declaration_identities(&mut self, program: &Program) {
        let nominals = self.checked_nominal_type_identities();
        let mut identities = BTreeMap::new();
        for declaration in &program.declarations {
            if let Some(binding) = self
                .type_info
                .declarations
                .function_bindings_by_span
                .get(&declaration.span)
            {
                let Some(canonical) = &binding.identity else { continue };
                let binders = match &declaration.node {
                    Declaration::Function(function) => binder_names(&function.type_params),
                    Declaration::Partial(partial) => self
                        .symbols
                        .lookup(&partial.name)
                        .and_then(|id| self.symbols.get(id))
                        .and_then(|symbol| match &symbol.kind {
                            SymbolKind::Function(function) => Some(function.type_params.clone()),
                            _ => None,
                        })
                        .unwrap_or_default(),
                    _ => continue,
                };
                if let Some(identity) = source_callable_identity(
                    canonical,
                    SourceInputs {
                        parameters: &binding.params,
                        ownership: self.type_info.declarations.ownership_contracts.get(canonical),
                        receiver_offset: 0,
                    },
                    &binding.return_type,
                    binding.is_async,
                    &binders,
                    self.declaration_nominal_scopes
                        .get(canonical)
                        .map_or(nominals.as_ref(), |scope| scope.as_ref()),
                    None,
                ) {
                    identities.insert(canonical.clone(), identity);
                }
                continue;
            }
            let Some((name, binders, methods)) = nominal_methods(&declaration.node) else {
                continue;
            };
            let Some(owner) = self
                .type_info
                .declarations
                .declaration_identities
                .get(&declaration.span)
            else {
                continue;
            };
            let Some(stable_owner) = StableDeclarationId::from_canonical(owner, None, None) else {
                continue;
            };
            let Some(symbol) = self.symbols.lookup(name).and_then(|id| self.symbols.get(id)) else {
                continue;
            };
            let Some((checked, overloads)) = checked_methods(&symbol.kind) else {
                continue;
            };
            for method in methods {
                let candidate = overloads
                    .and_then(|groups| groups.get(&method.node.name))
                    .and_then(|group| {
                        group.iter().find(|candidate| {
                            candidate
                                .identity
                                .as_ref()
                                .is_some_and(|identity| identity.declaration_span == method.span)
                        })
                    })
                    .or_else(|| checked.get(&method.node.name));
                let Some(candidate) = candidate else { continue };
                let Some(canonical) = &candidate.identity else { continue };
                let mut callable_parameters = candidate.params.clone();
                if let Some(receiver) = candidate.receiver {
                    let mut receiver_parameter =
                        CallableParam::named("self", ResolvedType::SelfType, crate::ast::ParamKind::Normal);
                    receiver_parameter.is_mut = receiver == Receiver::Mutable;
                    callable_parameters.insert(0, receiver_parameter);
                }
                let mut parameters = binder_names(binders);
                parameters.extend(candidate.type_params.iter().cloned());
                if let Some(identity) = source_callable_identity(
                    canonical,
                    SourceInputs {
                        parameters: &callable_parameters,
                        ownership: self.type_info.declarations.ownership_contracts.get(canonical),
                        receiver_offset: usize::from(candidate.receiver.is_some()),
                    },
                    &candidate.return_type,
                    candidate.is_async,
                    &parameters,
                    self.declaration_nominal_scopes
                        .get(canonical)
                        .map_or(nominals.as_ref(), |scope| scope.as_ref()),
                    Some(StableDeclarationContext {
                        owner: stable_owner.clone(),
                        binding_ordinal: 0,
                    }),
                ) {
                    identities.insert(canonical.clone(), identity);
                }
            }
        }
        self.type_info.declarations.source_identities = identities;
    }
}

/// The checked callable frame and its source slots; method ownership indices exclude the prepended receiver.
struct SourceInputs<'a> {
    parameters: &'a [CallableParam],
    ownership: Option<&'a CheckedOwnershipContract>,
    receiver_offset: usize,
}

/// Project one checked callable with explicit binder and owner context; unresolved types publish no portable key.
fn source_callable_identity(
    canonical: &CanonicalSymbolId,
    inputs: SourceInputs<'_>,
    return_type: &ResolvedType,
    is_async: bool,
    binders: &[String],
    nominals: &BTreeMap<String, CanonicalSymbolId>,
    owner: Option<StableDeclarationContext>,
) -> Option<StableDeclarationId> {
    let mut parameters = inputs
        .parameters
        .iter()
        .map(semantic_callable_param_from_resolved)
        .collect::<Vec<_>>();
    if let Some(contract) = inputs.ownership {
        if contract.schema_version != 1 {
            return None;
        }
        for mode in &contract.modes {
            let Some(slot) = mode.component.parameter else { continue };
            if !mode.component.children.is_empty() {
                continue;
            }
            let Some(access) = source_input_access(mode.mode) else {
                continue;
            };
            let parameter = parameters.get_mut(slot.checked_add(inputs.receiver_offset)?)?;
            parameter.input_access = access;
            parameter.ty = semantic_type_from_resolved(&mode.underlying);
        }
    }
    let signature = SourceSignatureContext { nominals, binders }.callable(
        &parameters,
        &semantic_type_from_resolved(return_type),
        is_async,
    )?;
    StableDeclarationId::from_canonical(canonical, Some(signature), owner)
}

/// Retain binder order from the checked declaration's lexical frame rather than a global traversal table.
fn binder_names(parameters: &[TypeParam]) -> Vec<String> {
    parameters.iter().map(|parameter| parameter.name.clone()).collect()
}

/// Select authored methods and their enclosing generic frame without resolving annotations again.
fn nominal_methods(declaration: &Declaration) -> Option<(&str, &[TypeParam], &[Spanned<MethodDecl>])> {
    Some(match declaration {
        Declaration::Model(owner) => (&owner.name, &owner.type_params, &owner.methods),
        Declaration::Class(owner) => (&owner.name, &owner.type_params, &owner.methods),
        Declaration::Enum(owner) => (&owner.name, &owner.type_params, &owner.methods),
        Declaration::Newtype(owner) => (&owner.name, &owner.type_params, &owner.methods),
        Declaration::Trait(owner) => (&owner.name, &owner.type_params, &owner.methods),
        _ => return None,
    })
}

/// Borrow the collector's checked method candidates, including same-owner overloads.
fn checked_methods(
    kind: &SymbolKind,
) -> Option<(&HashMap<String, MethodInfo>, Option<&HashMap<String, Vec<MethodInfo>>>)> {
    Some(match kind {
        SymbolKind::Type(TypeInfo::Model(owner)) => (&owner.methods, Some(&owner.method_overloads)),
        SymbolKind::Type(TypeInfo::Class(owner)) => (&owner.methods, Some(&owner.method_overloads)),
        SymbolKind::Type(TypeInfo::Enum(owner)) => (&owner.methods, Some(&owner.method_overloads)),
        SymbolKind::Type(TypeInfo::Newtype(owner)) => (&owner.methods, Some(&owner.method_overloads)),
        SymbolKind::Trait(owner) => (&owner.methods, None),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::TypeChecker;
    use std::collections::BTreeMap;

    /// Source identities remain inspectable while the explicit execution diagnostic deliberately refuses these bodies.
    fn signatures(source: &str) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        if let Err(errors) = checker.check_program(&program) {
            checker.errors = errors;
        }
        Ok(checker
            .type_info
            .declarations
            .source_identities
            .values()
            .filter_map(|identity| {
                Some((
                    identity.declaration_name.clone(),
                    identity.signature.as_ref()?.as_str().to_owned(),
                ))
            })
            .collect())
    }

    /// Canonical alias resolution preserves one key, while access modes on the same underlying scalar remain distinct.
    #[test]
    fn checked_input_modes_determine_source_signature() -> Result<(), Box<dyn std::error::Error>> {
        let mut keys = std::collections::BTreeSet::new();
        for mode in ["int", "Borrowed[int]", "BorrowedMut[int]", "Owned[int]"] {
            let source = format!("def take(value: {mode}) -> int:\n    return 1\n");
            let signatures = signatures(&source)?;
            keys.insert(signatures.get("take").ok_or("missing checked source identity")?.clone());
        }
        assert_eq!(keys.len(), 4);
        let direct = signatures("def take(value: Borrowed[int]) -> int:\n    return 1\n")?;
        let aliased = signatures("type Input = Borrowed[int]\ndef take(value: Input) -> int:\n    return 1\n")?;
        assert_eq!(direct.get("take"), aliased.get("take"));
        let parameter = incan_semantics_core::IncanCallableParam {
            name: Some("value".into()),
            ty: incan_semantics_core::IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Int),
            kind: incan_semantics_core::IncanCallableParamKind::Normal,
            has_default: false,
            is_partial_preset: false,
            is_mut: false,
            native_owned: false,
            input_access: incan_semantics_core::IncanInputAccess::Shared,
        };
        let nominals = BTreeMap::new();
        let expected = super::SourceSignatureContext {
            nominals: &nominals,
            binders: &[],
        }
        .callable(
            &[parameter],
            &incan_semantics_core::IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Int),
            false,
        )
        .ok_or("missing expected source signature")?;
        assert_eq!(direct.get("take").map(String::as_str), Some(expected.as_str()));
        Ok(())
    }
}
