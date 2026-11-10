//! Settle a concrete method's input permissions from the trait slot it implements before checking any body.

use super::TypeChecker;
use crate::ast::{Declaration, MethodDecl, Program, Span, TypeParam};
use crate::diagnostics::CompileError;
use crate::ownership_metadata::{AccessMode, CheckedOwnershipContract, OwnershipContract};
use crate::resolved_type_subst::{map_resolved_type_leaves, substitute_resolved_type, type_param_subst_map};
use crate::symbols::{MethodInfo, ResolvedType, SymbolKind, TypeBoundInfo, TypeInfo};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};
use std::collections::{BTreeMap, HashSet};

impl TypeChecker {
    /// Inherit only authenticated trait input facts. Ordinary methods retain their existing duckborrowing policy;
    /// this pass never infers access from an implementation body or changes the general shared-read exclusions.
    pub(super) fn inherit_trait_input_contracts(&mut self, program: &Program) {
        for declaration in &program.declarations {
            let (owner, methods) = match &declaration.node {
                Declaration::Model(owner) => (&owner.name, &owner.methods),
                Declaration::Class(owner) => (&owner.name, &owner.methods),
                Declaration::Enum(owner) => (&owner.name, &owner.methods),
                Declaration::Newtype(owner) => (&owner.name, &owner.methods),
                _ => continue,
            };
            let adoptions = self.input_contract_adoptions(owner);
            for method in methods {
                let Some((header, receiver)) = self.collected_ownership_method(owner, method) else {
                    continue;
                };
                let Some(identity) = header.identity.as_ref() else {
                    continue;
                };
                let selected = header
                    .trait_target
                    .as_ref()
                    .map_or(adoptions.as_slice(), std::slice::from_ref);
                let mut inherited = Vec::new();
                for adoption in selected {
                    let Some(entry) =
                        self.trait_method_entry_resolved_for_adoption(adoption, &method.node.name, method.span)
                    else {
                        continue;
                    };
                    let Some(contract) = entry
                        .info
                        .identity
                        .as_ref()
                        .and_then(|identity| self.type_info.declarations.ownership_contracts.get(identity))
                    else {
                        continue;
                    };
                    let substitutions = type_param_subst_map(
                        &entry.info.type_params,
                        &header
                            .type_params
                            .iter()
                            .cloned()
                            .map(ResolvedType::TypeVar)
                            .collect::<Vec<_>>(),
                    );
                    let mut candidate = Vec::new();
                    for mode in contract.modes.iter().filter(|mode| mode.component.parameter.is_some()) {
                        let Some(slot) = mode.component.parameter else { continue };
                        let Some(parameter) = entry.info.params.get(slot) else {
                            continue;
                        };
                        // The resolved adoption already substituted trait binders. Method binders and Self now
                        // close against this implementation, including implementations using different binder names.
                        if !mode.component.children.is_empty() {
                            self.errors.push(input_conflict(
                                "Nested trait input contracts are not executable",
                                method.span,
                            ));
                            continue;
                        }
                        let ResolvedType::Generic(_, arguments) = &parameter.ty else {
                            continue;
                        };
                        let [value] = arguments.as_slice() else { continue };
                        let value = substitute_resolved_type(value, &substitutions);
                        let value = self.substitute_self_in_resolved_type(value, &receiver);
                        candidate.push(AccessMode {
                            mode: mode.mode,
                            component: mode.component.clone(),
                            underlying: value,
                        });
                    }
                    if self.trait_input_shape_matches(&entry.info, &header, &candidate, &substitutions, &receiver) {
                        inherited.extend(candidate);
                    }
                }
                if inherited.is_empty() {
                    continue;
                }
                match self.merge_trait_inputs(identity, &header, inherited, method.span) {
                    Ok(contract) => {
                        let mut normalized = header.clone();
                        normalize_parameters(&mut normalized, &contract);
                        self.publish_inherited_method_header(owner, &normalized);
                        self.type_info
                            .declarations
                            .ownership_contracts
                            .insert(identity.clone(), contract);
                    }
                    Err(error) => self.errors.push(error),
                }
            }
        }
    }

    /// Select the implementation's trait slot by the ordinary value signature before merging permissions.
    /// Two instantiations of one trait may own different overloads; only matching slots can conflict on access.
    fn trait_input_shape_matches(
        &self,
        expected: &MethodInfo,
        found: &MethodInfo,
        inherited: &[AccessMode<ResolvedType>],
        substitutions: &std::collections::HashMap<String, ResolvedType>,
        receiver: &ResolvedType,
    ) -> bool {
        let mut expected = expected.clone();
        for parameter in &mut expected.params {
            parameter.ty =
                self.substitute_self_in_resolved_type(substitute_resolved_type(&parameter.ty, substitutions), receiver);
        }
        expected.return_type = self
            .substitute_self_in_resolved_type(substitute_resolved_type(&expected.return_type, substitutions), receiver);
        for mode in inherited {
            if let Some(parameter) = mode.component.parameter.and_then(|slot| expected.params.get_mut(slot)) {
                parameter.ty.clone_from(&mode.underlying);
            }
        }
        let mut found = found.clone();
        if let Some(contract) = found
            .identity
            .as_ref()
            .and_then(|identity| self.type_info.declarations.ownership_contracts.get(identity))
        {
            for mode in &contract.modes {
                if mode.component.children.is_empty() {
                    if let Some(parameter) = mode.component.parameter.and_then(|slot| found.params.get_mut(slot)) {
                        parameter.ty.clone_from(&mode.underlying);
                    }
                }
            }
        }
        self.method_sigs_compatible(&expected, &found)
    }

    /// Retain concrete generic trait adoptions, including a class's inherited obligations, without resolving their
    /// source annotations again outside their declaration scope. Cyclic bases are diagnosed by class checking.
    fn input_contract_adoptions(&self, owner: &str) -> Vec<TypeBoundInfo> {
        let mut result = Vec::new();
        let mut next = Some(owner.to_owned());
        let mut seen = HashSet::new();
        while let Some(owner) = next.take() {
            if !seen.insert(owner.clone()) {
                break;
            }
            let entries = match self.lookup_semantic_type_info(&owner) {
                Some(TypeInfo::Class(info)) => {
                    next = info.extends.clone();
                    &info.trait_adoptions
                }
                Some(TypeInfo::Model(info)) => &info.trait_adoptions,
                Some(TypeInfo::Enum(info)) => &info.trait_adoptions,
                Some(TypeInfo::Newtype(info)) => &info.trait_adoptions,
                _ => break,
            };
            for entry in entries {
                if !result.contains(entry) {
                    result.push(entry.clone());
                }
            }
        }
        result
    }

    /// Merge compatible slot contracts, refusing conflicting traits or an explicitly incompatible implementation.
    /// Result dependencies remain owned by the implementation's existing result contract; this pass adds inputs only.
    fn merge_trait_inputs(
        &self,
        identity: &CanonicalSymbolId,
        header: &MethodInfo,
        inherited: Vec<AccessMode<ResolvedType>>,
        span: Span,
    ) -> Result<CheckedOwnershipContract, CompileError> {
        let mut contract = self
            .type_info
            .declarations
            .ownership_contracts
            .get(identity)
            .cloned()
            .unwrap_or(OwnershipContract {
                schema_version: 1,
                modes: Vec::new(),
                result_source: None,
            });
        let mut slots = BTreeMap::new();
        for mode in inherited {
            let Some(slot) = mode.component.parameter else { continue };
            if let Some(previous) = slots.insert(slot, mode.clone()) {
                if previous != mode {
                    return Err(input_conflict(
                        "Adopted traits require conflicting input ownership for this method",
                        span,
                    ));
                }
            }
            let parameter = header
                .params
                .get(slot)
                .ok_or_else(|| input_conflict("Trait input slot is outside the implementation signature", span))?;
            if let Some(explicit) = contract
                .modes
                .iter()
                .find(|candidate| candidate.component.parameter == Some(slot))
            {
                if explicit != &mode {
                    return Err(input_conflict(
                        "Implementation input ownership differs from its trait contract",
                        span,
                    ));
                }
            } else {
                if !self.types_compatible(&mode.underlying, &parameter.ty)
                    || !self.types_compatible(&parameter.ty, &mode.underlying)
                {
                    return Err(input_conflict(
                        "Implementation input type differs from its trait contract",
                        span,
                    ));
                }
                contract.modes.push(mode);
            }
        }
        contract.modes.sort_by(|left, right| {
            (
                left.component.parameter.is_none(),
                left.component.parameter,
                &left.component.children,
            )
                .cmp(&(
                    right.component.parameter.is_none(),
                    right.component.parameter,
                    &right.component.children,
                ))
        });
        Ok(contract)
    }

    /// Publish the settled header to ordinary and overloaded lookup together. Canonical identity prevents a
    /// same-named sibling or inherited declaration from acquiring another method's permissions.
    fn publish_inherited_method_header(&mut self, owner: &str, header: &MethodInfo) {
        let Some(symbol) = self.symbols.lookup(owner).and_then(|id| self.symbols.get_mut(id)) else {
            return;
        };
        let (methods, overloads) = match &mut symbol.kind {
            SymbolKind::Type(TypeInfo::Model(info)) => (&mut info.methods, &mut info.method_overloads),
            SymbolKind::Type(TypeInfo::Class(info)) => (&mut info.methods, &mut info.method_overloads),
            SymbolKind::Type(TypeInfo::Enum(info)) => (&mut info.methods, &mut info.method_overloads),
            SymbolKind::Type(TypeInfo::Newtype(info)) => (&mut info.methods, &mut info.method_overloads),
            _ => return,
        };
        for method in methods.values_mut().chain(overloads.values_mut().flatten()) {
            if method.identity == header.identity {
                method.params.clone_from(&header.params);
            }
        }
    }

    /// The body source may omit a trait-owned mode. Reconstitute its settled callable metadata before binding locals
    /// so native lowering and published interfaces consume the same explicit wrapper and canonical access fact.
    pub(in crate::typechecker) fn inherited_method_parameter_types(
        &self,
        method: &MethodDecl,
        span: Span,
        parameters: Vec<ResolvedType>,
        owner_parameters: &[TypeParam],
        receiver: &ResolvedType,
    ) -> Vec<ResolvedType> {
        let identity = self
            .symbols
            .member_declaration_identity(&method.name, SemanticSourceTargetKind::Method, span);
        let Some(contract) = self.type_info.declarations.ownership_contracts.get(&identity) else {
            return parameters;
        };
        parameters
            .into_iter()
            .enumerate()
            .map(|(slot, ty)| {
                let mode = contract
                    .modes
                    .iter()
                    .find(|mode| mode.component.parameter == Some(slot) && mode.component.children.is_empty());
                let normalized = map_resolved_type_leaves(&ty, &|leaf| match leaf {
                    ResolvedType::Named(name)
                        if method
                            .type_params
                            .iter()
                            .chain(owner_parameters)
                            .any(|binder| binder.name == *name) =>
                    {
                        Some(ResolvedType::TypeVar(name.clone()))
                    }
                    ResolvedType::SelfType => Some(receiver.clone()),
                    _ => None,
                });
                match mode {
                    Some(mode) if normalized == mode.underlying => {
                        ResolvedType::Generic(mode.mode.as_str().into(), vec![ty])
                    }
                    _ => ty,
                }
            })
            .collect()
    }
}

/// Only a mode already merged into the canonical method contract may amend the method's parameter metadata.
fn normalize_parameters(header: &mut MethodInfo, contract: &CheckedOwnershipContract) {
    for mode in &contract.modes {
        let Some(slot) = mode.component.parameter else { continue };
        if !mode.component.children.is_empty() {
            continue;
        }
        if let Some(parameter) = header.params.get_mut(slot) {
            if parameter.ty == mode.underlying {
                parameter.ty = ResolvedType::Generic(mode.mode.as_str().into(), vec![parameter.ty.clone()]);
            }
        }
    }
}

/// Ownership disagreement is a source diagnostic at the implementing declaration, not a native ABI failure.
fn input_conflict(message: &str, span: Span) -> CompileError {
    CompileError::type_error(message.into(), span).with_stable_code("INCAN-T0123")
}
