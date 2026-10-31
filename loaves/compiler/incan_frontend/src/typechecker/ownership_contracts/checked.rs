//! Resolve explicit declarations into canonical slots and stored-field identities before refusing execution.

use super::TypeChecker;
use crate::ast::{FunctionDecl, MethodDecl, Param, ResultSource, Span, Spanned, Type};
use crate::diagnostics::CompileError;
use crate::ownership_metadata::{
    AccessMode, CheckedOwnershipContract, OwnershipContract, ResultDependency, SourceSlot, TypeComponent,
};
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::symbols::{ResolvedType, TypeInfo};
use incan_lang::lang::types::ownership;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolOrigin};
use std::collections::BTreeMap;

struct Signature<'a> {
    parameters: &'a [Spanned<Param>],
    parameter_types: &'a [ResolvedType],
    result: &'a Spanned<Type>,
    result_type: &'a ResolvedType,
    source: Option<&'a Spanned<ResultSource>>,
    receiver: Option<(&'a str, &'a ResolvedType)>,
}

impl TypeChecker {
    /// Capture the function's checked explicit contract under the same identity its callable binding owns.
    pub(in crate::typechecker) fn record_function_ownership(
        &mut self,
        function: &FunctionDecl,
        span: Span,
        parameters: &[ResolvedType],
        result: &ResolvedType,
    ) {
        let identity =
            self.symbols
                .module_declaration_identity(&function.name, SemanticSourceTargetKind::Function, span);
        self.record_ownership_signature(
            identity,
            Signature {
                parameters: &function.params,
                parameter_types: parameters,
                result: &function.return_type,
                result_type: result,
                source: function.result_source.as_ref(),
                receiver: None,
            },
        );
    }

    /// Abstract and concrete methods use the same slot and field-identity contract; a receiver is a distinct slot.
    pub(in crate::typechecker) fn record_method_ownership(
        &mut self,
        method: &MethodDecl,
        span: Span,
        parameters: &[ResolvedType],
        result: &ResolvedType,
        receiver_type: &ResolvedType,
    ) {
        let identity = self
            .symbols
            .member_declaration_identity(&method.name, SemanticSourceTargetKind::Method, span);
        self.record_ownership_signature(
            identity,
            Signature {
                parameters: &method.params,
                parameter_types: parameters,
                result: &method.return_type,
                result_type: result,
                source: method.result_source.as_ref(),
                receiver: method
                    .receiver_binding
                    .as_ref()
                    .map(|binding| (binding.node.as_str(), receiver_type)),
            },
        );
    }

    /// Retain structurally checked metadata even when execution is unavailable, without inventing a source proof.
    fn record_ownership_signature(&mut self, identity: CanonicalSymbolId, signature: Signature<'_>) {
        let mut contract = OwnershipContract {
            schema_version: 1,
            modes: Vec::new(),
            result_source: None,
        };
        for (slot, (parameter, ty)) in signature.parameters.iter().zip(signature.parameter_types).enumerate() {
            let mut aliases = BTreeMap::new();
            self.ownership_alias_identities(&parameter.node.ty.node, &mut aliases);
            self.collect_ownership_modes(
                ty,
                TypeComponent {
                    parameter: Some(slot),
                    children: Vec::new(),
                },
                &aliases,
                &mut contract,
            );
        }
        let mut aliases = BTreeMap::new();
        self.ownership_alias_identities(&signature.result.node, &mut aliases);
        self.collect_ownership_modes(
            signature.result_type,
            TypeComponent {
                parameter: None,
                children: Vec::new(),
            },
            &aliases,
            &mut contract,
        );
        if let Some(source) = signature.source {
            let root = source.node.binding.node.as_str();
            let selected = signature
                .parameters
                .iter()
                .position(|parameter| parameter.node.name == root)
                .and_then(|slot| {
                    signature
                        .parameter_types
                        .get(slot)
                        .map(|ty| (SourceSlot::Parameter(slot), ty))
                })
                .or_else(|| {
                    signature
                        .receiver
                        .filter(|(name, _)| *name == root)
                        .map(|(_, ty)| (SourceSlot::Receiver, ty))
                });
            let Some((slot, ty)) = selected else {
                self.ownership_source_error(
                    "Result source must name this declaration's parameter or receiver",
                    source.node.binding.span,
                );
                return;
            };
            let Some(fields) = self.resolve_ownership_source_fields(ty, source) else {
                return;
            };
            contract.result_source = Some(ResultDependency { slot, fields });
            self.errors.push(
                CompileError::type_error(
                    "Borrowed result-source contracts are recognized but not executable yet".into(),
                    source.span,
                )
                .with_stable_code("INCAN-T0123")
                .with_note(
                    "A resolved source declaration is not proof that the executable body retains only that source",
                ),
            );
        }
        if contract.modes.is_empty() && contract.result_source.is_none() {
            return;
        }
        if let Err(reason) = contract.validate(signature.parameters.len(), signature.receiver.is_some()) {
            self.ownership_source_error(reason, signature.result.span);
            return;
        }
        self.type_info
            .declarations
            .ownership_contracts
            .insert(identity, contract);
    }

    /// Keep alias declaration identities from being reinterpreted after a consumer shadows a prelude spelling.
    fn ownership_alias_identities(&self, annotation: &Type, identities: &mut BTreeMap<String, CanonicalSymbolId>) {
        match annotation {
            Type::Simple(name) | Type::Generic(name, _) => {
                if let Some(alias) = self.type_aliases.get(name) {
                    identities.extend(alias.nominal_identities.clone());
                }
                if let Type::Generic(_, arguments) = annotation {
                    for argument in arguments {
                        self.ownership_alias_identities(&argument.node, identities);
                    }
                }
            }
            Type::DottedGeneric(_, arguments) | Type::Tuple(arguments) => {
                for argument in arguments {
                    self.ownership_alias_identities(&argument.node, identities);
                }
            }
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => {
                self.ownership_alias_identities(&inner.node, identities)
            }
            Type::Function(parameters, result) => {
                for parameter in parameters {
                    self.ownership_alias_identities(&parameter.node, identities);
                }
                self.ownership_alias_identities(&result.node, identities);
            }
            _ => {}
        }
    }

    /// Visit checked type components in declaration order, preserving mode identity independently of presentation.
    fn collect_ownership_modes(
        &self,
        ty: &ResolvedType,
        component: TypeComponent,
        aliases: &BTreeMap<String, CanonicalSymbolId>,
        contract: &mut CheckedOwnershipContract,
    ) {
        let children: Vec<&ResolvedType> = match ty {
            ResolvedType::Generic(name, arguments) => {
                let mode = match aliases.get(name) {
                    Some(identity) if identity.origin == SymbolOrigin::Builtin => {
                        ownership::from_str(&identity.declaration_name)
                    }
                    Some(_) => None,
                    None => self.explicit_ownership_mode(name),
                };
                if let Some(mode) = mode
                    && let [underlying] = arguments.as_slice()
                {
                    contract.modes.push(AccessMode {
                        mode,
                        component: component.clone(),
                        underlying: underlying.clone(),
                    });
                }
                arguments.iter().collect()
            }
            ResolvedType::Tuple(elements) => elements.iter().collect(),
            ResolvedType::Function(parameters, result) => parameters
                .iter()
                .map(|parameter| &parameter.ty)
                .chain(std::iter::once(result.as_ref()))
                .collect(),
            ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner)
            | ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::TypeToken(inner) => vec![inner],
            ResolvedType::FrozenDict(key, value) => vec![key, value],
            _ => Vec::new(),
        };
        for (slot, child) in children.into_iter().enumerate() {
            let mut nested = component.clone();
            nested.children.push(slot);
            self.collect_ownership_modes(child, nested, aliases, contract);
        }
    }

    /// Resolve only stored fields through the checked nominal table, substituting the owner's generic arguments.
    fn resolve_ownership_source_fields(
        &mut self,
        root: &ResolvedType,
        source: &Spanned<ResultSource>,
    ) -> Option<Vec<CanonicalSymbolId>> {
        let mut ty = root.clone();
        let mut fields = Vec::new();
        for field in &source.node.fields {
            while let ResolvedType::Generic(name, arguments) = &ty {
                if self.explicit_ownership_mode(name).is_none() || arguments.len() != 1 {
                    break;
                }
                ty = arguments[0].clone();
            }
            let selected = match &ty {
                ResolvedType::Named(name) | ResolvedType::Generic(name, _) => {
                    let (parameters, fields) = match self.lookup_type_info(name) {
                        Some(TypeInfo::Model(owner)) => (&owner.type_params, &owner.fields),
                        Some(TypeInfo::Class(owner)) => (&owner.type_params, &owner.fields),
                        _ => {
                            self.ownership_source_error(
                                "Result source field requires a checked model or class",
                                field.span,
                            );
                            return None;
                        }
                    };
                    fields.get(&field.node).map(|info| {
                        let arguments = match &ty {
                            ResolvedType::Generic(_, arguments) => arguments.as_slice(),
                            _ => &[],
                        };
                        (
                            info.identity.clone(),
                            substitute_resolved_type(&info.ty, &type_param_subst_map(parameters, arguments)),
                        )
                    })
                }
                _ => None,
            };
            let Some((Some(identity), next_type)) = selected else {
                self.ownership_source_error(
                    "Result source path must name an existing stored field with a checked declaration identity",
                    field.span,
                );
                return None;
            };
            fields.push(identity);
            ty = next_type;
        }
        Some(fields)
    }

    /// Report malformed source relationships consistently for direct and alias-expanded signatures.
    fn ownership_source_error(&mut self, message: &str, span: Span) {
        self.errors
            .push(CompileError::type_error(message.to_owned(), span).with_stable_code("INCAN-T0124"));
    }
}
