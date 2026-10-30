//! Publish compiler-owned source signatures after collection, independently of executable representations.

use std::collections::{BTreeMap, HashMap};

use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::stable_identity::source_signature::SourceSignatureContext;
use incan_semantics_core::stable_identity::{StableDeclarationContext, StableDeclarationId};

use crate::ast::{Declaration, MethodDecl, Program, Receiver, Spanned, TypeParam};
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
                    &binding.params,
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
                            candidate.identity.as_ref().is_some_and(|identity| {
                                identity.declaration_span.start == method.span.start
                                    && identity.declaration_span.end == method.span.end
                            })
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
                    &callable_parameters,
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

/// Project one checked callable with explicit binder and owner context; unresolved types publish no portable key.
fn source_callable_identity(
    canonical: &CanonicalSymbolId,
    parameters: &[CallableParam],
    return_type: &ResolvedType,
    is_async: bool,
    binders: &[String],
    nominals: &BTreeMap<String, CanonicalSymbolId>,
    owner: Option<StableDeclarationContext>,
) -> Option<StableDeclarationId> {
    let parameters = parameters
        .iter()
        .map(semantic_callable_param_from_resolved)
        .collect::<Vec<_>>();
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
