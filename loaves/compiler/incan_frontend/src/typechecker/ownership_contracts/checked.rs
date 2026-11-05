//! Resolve explicit declarations into canonical slots and stored-field identities before refusing execution.

use super::TypeChecker;
use crate::ast::{Declaration, FunctionDecl, MethodDecl, Param, Program, Receiver, ResultSource, Span, Spanned, Type};
use crate::diagnostics::CompileError;
use crate::ownership_metadata::{
    AccessMode, CheckedOwnershipContract, OwnershipContract, ResultDependency, SourceSlot, TypeComponent,
};
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::symbols::{CallableParam, MethodInfo, ResolvedType, TypeInfo};
use incan_lang::lang::types::ownership;
use incan_semantics_core::{CanonicalSymbolId, IncanInputAccess, SemanticSourceTargetKind, SymbolOrigin};
use std::collections::BTreeMap;

struct Signature<'a> {
    parameters: &'a [Spanned<Param>],
    parameter_types: &'a [ResolvedType],
    result: &'a Spanned<Type>,
    result_type: &'a ResolvedType,
    source: Option<&'a Spanned<ResultSource>>,
    receiver: Option<(&'a str, &'a ResolvedType)>,
    receiver_mode: Option<Receiver>,
}

impl TypeChecker {
    /// Retain successful selected calls in the isolated body writer. Ordinary candidates are discarded unless the
    /// completed body independently needs explicit-input analysis. Candidate speculation is discarded with its
    /// normal call artifacts, and a caller without annotations still owes its callee's explicit contract.
    pub(in crate::typechecker) fn record_explicit_input_call(
        &mut self,
        identity: Option<&CanonicalSymbolId>,
        parameters: &[CallableParam],
        receiver: Option<Receiver>,
        span: Span,
    ) {
        if let Some(call) = self.explicit_input_call_fact(identity, parameters, receiver) {
            self.type_info.protocols.explicit_input_calls.insert(span, call);
        }
    }

    /// Retain a method only after dispatch selected it, using the completed parameter inventory. Diagnostic
    /// fallback validation can inspect a candidate but must not publish it as the source call's ownership target.
    /// A call site whose parameters needed no snapshot uses the selected declaration's own inventory.
    pub(in crate::typechecker) fn record_selected_method_input_call(
        &mut self,
        identity: Option<&CanonicalSymbolId>,
        receiver: Option<Receiver>,
        declared: &[CallableParam],
        span: Span,
    ) {
        let parameters = self.type_info.call_site_callable_params(span).unwrap_or(declared);
        let call = self.explicit_input_call_fact(identity, parameters, receiver);
        if let Some(call) = call {
            self.type_info.protocols.explicit_input_calls.insert(span, call);
        }
    }

    /// Retain a successful call through a callable parameter. Its canonical binding identifies the value read by
    /// the call; a local closure keeps the existing capture-aware route instead of replacing it with this frame.
    pub(in crate::typechecker) fn record_indirect_input_call(
        &mut self,
        callee_span: Span,
        parameters: &[CallableParam],
        span: Span,
    ) {
        let Some(owner) = self.current_body_identity().cloned() else {
            return;
        };
        let Some(callee) = self.type_info.resolved_identity(callee_span).cloned() else {
            return;
        };
        if callee.kind != SemanticSourceTargetKind::Parameter {
            return;
        }
        self.type_info.protocols.explicit_input_calls.insert(
            span,
            super::InputCall {
                kind: super::InputCallKind::Indirect,
                owner: Some(owner),
                callee,
                parameters: parameters.to_vec(),
                receiver: None,
            },
        );
    }

    /// Capture successful selection once. Plain free calls stay provisional until the isolated body writer has
    /// independently established an explicit obligation; ordinary methods and noncanonical headers stay staged.
    fn explicit_input_call_fact(
        &self,
        identity: Option<&CanonicalSymbolId>,
        parameters: &[CallableParam],
        receiver: Option<Receiver>,
    ) -> Option<super::InputCall> {
        let callee = identity?;
        let kind = if self
            .type_info
            .declarations
            .ownership_contracts
            .get(callee)
            .is_some_and(|contract| {
                contract.result_source.is_some() || contract.modes.iter().any(|mode| mode.component.parameter.is_some())
            }) {
            super::InputCallKind::Explicit
        } else {
            if self.current_body_identity().is_none() || receiver.is_some() {
                return None;
            }
            super::InputCallKind::Ordinary
        };
        let call = super::InputCall {
            kind,
            owner: self.current_body_identity().cloned(),
            callee: callee.clone(),
            parameters: parameters.to_vec(),
            receiver,
        };
        self.type_info.selected_input_contract(&call).ok()?;
        Some(call)
    }

    /// Project only the direct input modes owned by the selected declaration into call-local value types.
    /// Published parameter metadata remains unchanged, and the canonical contract stays available for access
    /// checking. Wrapper shape disagreement is refused instead of guessing from a type's display spelling.
    pub(in crate::typechecker) fn ownership_call_value_parameters(
        &self,
        identity: Option<&CanonicalSymbolId>,
        parameters: &[CallableParam],
        receiver: Option<&ResolvedType>,
        span: Span,
    ) -> Result<Option<Vec<CallableParam>>, CompileError> {
        let Some(contract) =
            identity.and_then(|identity| self.type_info.declarations.ownership_contracts.get(identity))
        else {
            return Ok(None);
        };
        // Method parameter checking has already closed Self against this selected receiver. Apply exactly the
        // same substitution to its ownership payload; Self is never an unconstrained wildcard in validation.
        let receiver_contract = receiver.map(|receiver| {
            let mut projected = contract.clone();
            for mode in &mut projected.modes {
                mode.underlying = self.substitute_self_in_resolved_type(mode.underlying.clone(), receiver);
            }
            projected
        });
        let contract = receiver_contract.as_ref().unwrap_or(contract);
        let mut output = None;
        for mode in &contract.modes {
            let Some(slot) = mode.component.parameter else { continue };
            if !mode.component.children.is_empty() || mode.mode == ownership::OwnershipMode::Reference {
                continue;
            }
            let parameter = parameters.get(slot).ok_or_else(|| {
                CompileError::type_error(
                    "Selected ownership parameter slot is outside the signature".into(),
                    span,
                )
                .with_stable_code("INCAN-T0123")
            })?;
            let underlying = crate::ownership_metadata::input_value_type(Some(contract), slot, &parameter.ty)
                .map_err(|reason| CompileError::type_error(reason.into(), span).with_stable_code("INCAN-T0123"))?;
            let projected = output.get_or_insert_with(|| parameters.to_vec());
            projected[slot].ty = underlying.clone();
        }
        Ok(output)
    }

    /// Separate a direct input obligation from the value seen by its body. Canonical alias provenance is retained;
    /// a user type with the same spelling is not stripped. Nested and returned access contracts stay staged.
    pub(in crate::typechecker) fn explicit_input_contract(
        &self,
        annotation: &Type,
        ty: &ResolvedType,
    ) -> Option<AccessMode<ResolvedType>> {
        self.explicit_root_contract(annotation, ty)
            .filter(|mode| mode.mode != ownership::OwnershipMode::Reference)
    }

    /// Capture a canonical outer access request before a binding changes lookup. Callers decide which authoring
    /// roles their current stage supports; an unsupported mode is still a fact and cannot be silently erased.
    pub(in crate::typechecker) fn explicit_root_contract(
        &self,
        annotation: &Type,
        ty: &ResolvedType,
    ) -> Option<AccessMode<ResolvedType>> {
        let ResolvedType::Generic(name, arguments) = ty else {
            return None;
        };
        let [underlying] = arguments.as_slice() else {
            return None;
        };
        let mut aliases = BTreeMap::new();
        self.ownership_alias_identities(annotation, &mut aliases);
        let mode = match aliases.get(name) {
            Some(identity) if identity.origin == SymbolOrigin::Builtin => {
                ownership::from_str(&identity.declaration_name)
            }
            Some(_) => None,
            None => self.explicit_ownership_mode(name),
        };
        let mode = mode?;
        Some(AccessMode {
            mode,
            component: TypeComponent {
                parameter: Some(0),
                children: Vec::new(),
            },
            underlying: underlying.clone(),
        })
    }

    /// Read an exact declaration slot instead of looking up an annotation name again in the parameter scope.
    /// Earlier parameters may legitimately shadow a prelude spelling after the signature has been resolved.
    pub(in crate::typechecker) fn checked_input_contract(
        &self,
        declaration: &CanonicalSymbolId,
        slot: usize,
    ) -> Option<&AccessMode<ResolvedType>> {
        self.type_info
            .declarations
            .ownership_contracts
            .get(declaration)?
            .modes
            .iter()
            .find(|mode| {
                mode.component.parameter == Some(slot)
                    && mode.component.children.is_empty()
                    && mode.mode != ownership::OwnershipMode::Reference
            })
    }

    /// Prepare method contracts and complete valid free-function contracts before any caller body is checked. Failed
    /// signatures remain unavailable; their declaration-owned check reports the error at its normal source anchor.
    /// This uses the same canonical inventory as publication, including distinct overload declaration spans.
    pub(in crate::typechecker) fn collect_function_ownership_headers(&mut self, program: &Program) {
        self.collect_method_ownership_headers(program);
        for declaration in &program.declarations {
            let Declaration::Function(function) = &declaration.node else {
                continue;
            };
            let Some(binding) = self
                .type_info
                .declarations
                .function_bindings_by_span
                .get(&declaration.span)
            else {
                continue;
            };
            let Some(identity) = binding.identity.clone() else {
                continue;
            };
            let parameters: Vec<_> = binding.params.iter().map(|parameter| parameter.ty.clone()).collect();
            let signature = Signature {
                parameters: &function.params,
                parameter_types: &parameters,
                result: &function.return_type,
                result_type: &binding.return_type,
                source: function.result_source.as_ref(),
                receiver: None,
                receiver_mode: None,
            };
            if let Ok(Some(contract)) = self.checked_ownership_signature(&signature) {
                self.type_info
                    .declarations
                    .ownership_contracts
                    .insert(identity, contract);
            }
        }
    }

    /// Prepare nominal and trait method contracts before checking callers that precede their owner declaration.
    /// The collected method's canonical identity and full declaration span select the header, including overloads
    /// and generated expansions. Failed signatures remain owned by the normal declaration diagnostic path.
    fn collect_method_ownership_headers(&mut self, program: &Program) {
        for declaration in &program.declarations {
            let (owner, methods) = match &declaration.node {
                Declaration::Model(owner) => (&owner.name, &owner.methods),
                Declaration::Class(owner) => (&owner.name, &owner.methods),
                Declaration::Enum(owner) => (&owner.name, &owner.methods),
                Declaration::Newtype(owner) => (&owner.name, &owner.methods),
                Declaration::Trait(owner) => (&owner.name, &owner.methods),
                _ => continue,
            };
            for method in methods {
                let Some((header, receiver_type)) = self.collected_ownership_method(owner, method) else {
                    continue;
                };
                let Some(identity) = header.identity else { continue };
                let parameters: Vec<_> = header.params.iter().map(|parameter| parameter.ty.clone()).collect();
                let signature = Signature {
                    parameters: &method.node.params,
                    parameter_types: &parameters,
                    result: &method.node.return_type,
                    result_type: &header.return_type,
                    source: method.node.result_source.as_ref(),
                    receiver: method
                        .node
                        .receiver_binding
                        .as_ref()
                        .map(|binding| (binding.node.as_str(), &receiver_type)),
                    receiver_mode: method.node.receiver,
                };
                if let Ok(Some(contract)) = self.checked_ownership_signature(&signature) {
                    self.type_info
                        .declarations
                        .ownership_contracts
                        .insert(identity, contract);
                }
            }
        }
    }

    /// Borrow the collected owner's inventories, then copy only the exact source method header needed by the
    /// signature checker. Inherited, aliased and same-spelled methods cannot replace the authored declaration.
    fn collected_ownership_method(
        &self,
        owner: &str,
        source: &Spanned<MethodDecl>,
    ) -> Option<(MethodInfo, ResolvedType)> {
        let (parameters, methods, overloads) = match self.lookup_semantic_type_info(owner) {
            Some(TypeInfo::Model(info)) => (&info.type_params, &info.methods, Some(&info.method_overloads)),
            Some(TypeInfo::Class(info)) => (&info.type_params, &info.methods, Some(&info.method_overloads)),
            Some(TypeInfo::Enum(info)) => (&info.type_params, &info.methods, Some(&info.method_overloads)),
            Some(TypeInfo::Newtype(info)) => (&info.type_params, &info.methods, Some(&info.method_overloads)),
            _ => {
                let info = self.lookup_semantic_trait_info(owner)?;
                (&info.type_params, &info.methods, None)
            }
        };
        let exact = |method: &&MethodInfo| {
            method.identity.as_ref().is_some_and(|identity| {
                identity.kind == SemanticSourceTargetKind::Method
                    && identity.declaration_name == source.node.name
                    && identity.declaration_span == source.span
            })
        };
        let header = overloads
            .and_then(|inventory| inventory.get(&source.node.name))
            .and_then(|methods| methods.iter().find(exact))
            .or_else(|| methods.get(&source.node.name).filter(exact))?;
        let receiver = if parameters.is_empty() {
            ResolvedType::Named(owner.into())
        } else {
            ResolvedType::Generic(
                owner.into(),
                parameters.iter().cloned().map(ResolvedType::TypeVar).collect(),
            )
        };
        Some((header.clone(), receiver))
    }

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
                receiver_mode: None,
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
                receiver_mode: method.receiver,
            },
        );
    }

    /// Retain structurally checked metadata even when the body is refused, without inventing a source proof.
    fn record_ownership_signature(&mut self, identity: CanonicalSymbolId, signature: Signature<'_>) {
        if !self.type_info.declarations.ownership_contracts.contains_key(&identity) {
            match self.checked_ownership_signature(&signature) {
                Ok(Some(contract)) => {
                    self.type_info
                        .declarations
                        .ownership_contracts
                        .insert(identity.clone(), contract);
                }
                Ok(None) => return,
                Err(error) => {
                    self.errors.push(error);
                    return;
                }
            }
        }
        if signature.source.is_some()
            && let Some(contract) = self.type_info.declarations.ownership_contracts.get(&identity).cloned()
            && let Err(error) = self.check_returned_view_header(&contract, &signature)
        {
            self.errors.push(error);
        }
    }

    /// A returned view needs a source that grants its access and holds its value type. Owned and ordinary inputs
    /// cannot back a view, and a shared source cannot supply exclusive access.
    fn check_returned_view_header(
        &self,
        contract: &CheckedOwnershipContract,
        signature: &Signature<'_>,
    ) -> Result<(), CompileError> {
        let Some(source) = signature.source else { return Ok(()) };
        if let Some((_, dependency)) =
            crate::ownership_metadata::owned_result_source(Some(contract)).map_err(|reason| {
                CompileError::type_error(
                    format!("Returned dependency cannot be proved: {reason}"),
                    signature.result.span,
                )
                .with_stable_code("INCAN-T0123")
            })?
        {
            let source_type = match dependency.slot {
                SourceSlot::Receiver => signature
                    .receiver
                    .map(|(_, receiver)| receiver.clone())
                    .ok_or_else(|| {
                        Self::ownership_source_error("Result source receiver is absent", source.node.binding.span)
                    })?,
                SourceSlot::Parameter(slot) => {
                    let input = contract
                        .modes
                        .iter()
                        .find(|mode| mode.component.parameter == Some(slot) && mode.component.children.is_empty());
                    match input.map(|mode| mode.mode) {
                        Some(ownership::OwnershipMode::Borrowed | ownership::OwnershipMode::BorrowedMut) => {
                            input.map(|mode| mode.underlying.clone()).ok_or_else(|| {
                                Self::ownership_source_error(
                                    "An owned result's source must be a borrowed input",
                                    source.node.binding.span,
                                )
                            })?
                        }
                        _ => {
                            return Err(Self::ownership_source_error(
                                "An owned result's source must be a Borrowed[T] or BorrowedMut[T] input",
                                source.node.binding.span,
                            ));
                        }
                    }
                }
            };
            self.resolve_ownership_source_fields(&source_type, source)?;
            return Ok(());
        }
        let view = crate::ownership_metadata::returned_view(Some(contract))
            .map_err(|reason| {
                CompileError::type_error(
                    format!("Returned access cannot be proved: {reason}"),
                    signature.result.span,
                )
                .with_stable_code("INCAN-T0123")
            })?
            .ok_or_else(|| {
                Self::ownership_source_error(
                    "A result source requires a Borrowed[T] or Reference[T] result",
                    source.span,
                )
            })?;
        // A carried view is one payload of a `Result` result, the payload of an `Option` result or one element of a
        // tuple result; no other type carries one.
        if let Some(payload) = view.carried
            && !(signature.result_type.is_result()
                || (signature.result_type.is_option() && payload == 0)
                || signature
                    .result_type
                    .tuple_elements()
                    .is_some_and(|elements| payload < elements.len()))
        {
            return Err(Self::ownership_source_error(
                "A view inside a result must be a payload of a Result, the payload of an Option or an element of a \
                 tuple",
                signature.result.span,
            ));
        }
        let exclusive = view.access == IncanInputAccess::Exclusive;
        let source_type = match view.source.slot {
            SourceSlot::Receiver => {
                let (_, receiver) = signature.receiver.ok_or_else(|| {
                    Self::ownership_source_error("Result source receiver is absent", source.node.binding.span)
                })?;
                if exclusive && signature.receiver_mode != Some(Receiver::Mutable) {
                    return Err(Self::ownership_source_error(
                        "Reference[T] requires exclusive access to its source; declare the receiver `mut self`",
                        source.node.binding.span,
                    ));
                }
                receiver.clone()
            }
            SourceSlot::Parameter(slot) => {
                let input = contract
                    .modes
                    .iter()
                    .find(|mode| mode.component.parameter == Some(slot) && mode.component.children.is_empty());
                let refusal = match input.map(|mode| mode.mode) {
                    Some(ownership::OwnershipMode::BorrowedMut) => None,
                    Some(ownership::OwnershipMode::Borrowed) if !exclusive => None,
                    Some(ownership::OwnershipMode::Borrowed) => {
                        Some("Reference[T] requires exclusive access to its source; a Borrowed[T] input is shared")
                    }
                    Some(ownership::OwnershipMode::Owned) => {
                        Some("An Owned[T] input is consumed by the call; a returned view cannot borrow from it")
                    }
                    _ => Some("A returned view's source must be a Borrowed[T] or BorrowedMut[T] input"),
                };
                match (refusal, input) {
                    (None, Some(input)) => input.underlying.clone(),
                    (refusal, _) => {
                        return Err(Self::ownership_source_error(
                            refusal.unwrap_or("A returned view's source must be a borrowed input"),
                            source.node.binding.span,
                        ));
                    }
                }
            }
        };
        let (_, held) = self.resolve_ownership_source_fields(&source_type, source)?;
        let returned = match signature.receiver {
            Some((_, receiver)) => self.substitute_self_in_resolved_type(view.mode.underlying.clone(), receiver),
            None => view.mode.underlying.clone(),
        };
        if returned != held {
            return Err(Self::ownership_source_error(
                &format!("Returned view type '{returned}' differs from its source's type '{held}'"),
                signature.result.span,
            ));
        }
        Ok(())
    }

    /// The value a body returns when its declaration's result is a direct returned view; otherwise the result.
    /// A malformed relation keeps the written result; its declaration check reports the refusal.
    pub(in crate::typechecker) fn returned_value_type(
        &self,
        identity: &CanonicalSymbolId,
        result: &ResolvedType,
    ) -> ResolvedType {
        let contract = self.type_info.declarations.ownership_contracts.get(identity);
        crate::ownership_metadata::result_value_type(contract, result).unwrap_or_else(|_| result.clone())
    }

    /// Project a selected declaration's returned view into the call's value type, as for its inputs. The published
    /// result metadata keeps the mode; the call's relation is checked against the declaration's contract.
    pub(in crate::typechecker) fn ownership_call_value_result(
        &self,
        identity: Option<&CanonicalSymbolId>,
        result: &ResolvedType,
        span: Span,
    ) -> Result<Option<ResolvedType>, CompileError> {
        let Some(contract) =
            identity.and_then(|identity| self.type_info.declarations.ownership_contracts.get(identity))
        else {
            return Ok(None);
        };
        let refusal = |reason: &str| CompileError::type_error(reason.into(), span).with_stable_code("INCAN-T0123");
        if crate::ownership_metadata::returned_view(Some(contract))
            .map_err(refusal)?
            .is_none()
            && crate::ownership_metadata::owned_result_source(Some(contract))
                .map_err(refusal)?
                .is_none()
        {
            return Ok(None);
        }
        crate::ownership_metadata::result_value_type(Some(contract), result)
            .map(Some)
            .map_err(refusal)
    }

    /// Resolve immutable header obligations without publishing diagnostics or consulting a callee's statements.
    /// Declaration checking owns refusals; successful contracts can be consumed regardless of source order.
    fn checked_ownership_signature(
        &self,
        signature: &Signature<'_>,
    ) -> Result<Option<CheckedOwnershipContract>, CompileError> {
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
                return Err(Self::ownership_source_error(
                    "Result source must name this declaration's parameter or receiver",
                    source.node.binding.span,
                ));
            };
            let (fields, _) = self.resolve_ownership_source_fields(ty, source)?;
            contract.result_source = Some(ResultDependency { slot, fields });
        }
        if contract.modes.is_empty() && contract.result_source.is_none() {
            return Ok(None);
        }
        if let Err(reason) = contract.validate(signature.parameters.len(), signature.receiver.is_some()) {
            return Err(Self::ownership_source_error(reason, signature.result.span));
        }
        Ok(Some(contract))
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
        &self,
        root: &ResolvedType,
        source: &Spanned<ResultSource>,
    ) -> Result<(Vec<CanonicalSymbolId>, ResolvedType), CompileError> {
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
                            return Err(Self::ownership_source_error(
                                "Result source field requires a checked model or class",
                                field.span,
                            ));
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
                return Err(Self::ownership_source_error(
                    "Result source path must name an existing stored field with a checked declaration identity",
                    field.span,
                ));
            };
            fields.push(identity);
            ty = next_type;
        }
        Ok((fields, ty))
    }

    /// Report malformed source relationships consistently for direct and alias-expanded signatures.
    fn ownership_source_error(message: &str, span: Span) -> CompileError {
        CompileError::type_error(message.to_owned(), span).with_stable_code("INCAN-T0124")
    }
}
