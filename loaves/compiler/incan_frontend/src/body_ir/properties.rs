//! Retain checked computed getters and value-producing member reads without changing legacy property lowering.

use std::collections::HashMap;

use super::args::fixed_elements;
use super::{
    AbiV0RuntimeRequirement, BodyBuilder, BodyIrLoweringFacts, CanonicalSymbolId, CompilerNodeId,
    IncanCallableParamKind, IncanType, SemanticSourceTargetKind, ast, bir, hir_span, parameter_types,
};

/// Build one concrete immutable getter body from its checked signature and exact implicit receiver provenance.
/// Abstract requirements contribute no body; missing checked facts remain explicit unsupported body statements.
pub(super) fn lower_getter_body(
    property: &ast::Spanned<ast::PropertyDecl>,
    owner: &str,
    receiver_ty: &IncanType,
    facts: &BodyIrLoweringFacts<'_, '_>,
) -> Option<bir::Body> {
    let source = property.node.body.as_ref()?;
    let checked_receiver = facts.type_info.qualify_semantic_type(receiver_ty.clone());
    let receiver_ty = &checked_receiver;

    // ---- Checked getter frame ----
    let binding = facts
        .type_info
        .declarations
        .body_ir_property_bindings
        .get(&(property.span.start, property.span.end));
    let parameters = binding
        .map(|binding| binding.owner_parameters.as_slice())
        .unwrap_or(&[]);
    let returned = binding
        .map(|binding| parameter_types::checked_semantic_type(&binding.callable.return_type, facts.type_info))
        .map(|ty| parameter_types::retain_parameter_type(ty, parameters))
        .unwrap_or(IncanType::Unknown);
    let mut builder = BodyBuilder::new(facts, returned.clone());
    builder.type_parameters = parameters.to_vec();
    let span = hir_span(property.span);
    let scope = builder.new_scope(None, span);
    let receiver = builder.declare_receiver_local(receiver_ty.clone(), false, scope, span);

    // ---- Getter body ----
    let mut stmts = Vec::new();
    if binding.is_none() || binding.is_some_and(|binding| binding.receiver_identity.is_none()) {
        builder.push_unsupported_stmt(
            "computed property without a checked signature or receiver identity".into(),
            span,
            &mut stmts,
        );
    } else {
        builder.lower_block_into(source, scope, &mut stmts);
        builder.insert_scope_drops(&mut stmts, scope);
    }
    if builder
        .locals
        .iter()
        .any(|local| !local.ty.abi_v0_facts().ownership.is_trivially_copy())
    {
        builder.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
    }

    // ---- Retained callable ----
    let named_type_identities =
        builder.finish_type_identities(facts.type_info.declarations.named_type_identities.clone());
    Some(bir::Body {
        trait_default_instantiation: None,
        nominal_instantiation: Vec::new(),
        decl_id: CompilerNodeId::declaration(facts.module_identity, &format!("{owner}::{}", property.node.name)),
        direct_call_id: CompilerNodeId::declaration_span(facts.module_identity, property.span.start, property.span.end),
        canonical: binding.and_then(|binding| binding.callable.identity.clone()),
        name: property.node.name.clone(),
        span,
        type_parameters: Vec::new(),
        return_type: returned,
        callable_representation: Some(bir::CallableRepresentation {
            function_pointer_locals: builder.function_pointer_locals,
            closure_holding_locals: builder.closure_holding_locals,
            closure_holding_parameters: Vec::new(),
            closure_holding_return: facts
                .type_info
                .is_closure_returning_type(property.node.return_type.span),
        }),
        named_type_identities,
        params: vec![bir::CallableParam {
            local: receiver,
            kind: IncanCallableParamKind::Normal,
            name: "self".into(),
            ty: receiver_ty.clone(),
            span,
            default: bir::CallableParamDefault::Required,
            mutable: false,
        }],
        param_locals: vec![receiver],
        locals: builder.locals,
        scopes: builder.scopes,
        block: bir::Block { scope, stmts },
        runtime_requirements: builder.runtime_requirements,
        panic_facts: builder.panic_facts,
        is_async: false,
        extern_delegation: None,
        native_attributes: Vec::new(),
    })
}

impl BodyBuilder<'_, '_> {
    /// Lower a checker-classified property read as a canonical getter call with an immutable receiver.
    /// An unclassified field remains storage; a classified but unproven getter never falls back to a field projection.
    pub(super) fn lower_property_read(
        &mut self,
        base: &ast::Spanned<ast::Expr>,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let source_span = hir_span(span);
        let Some(access) = self.type_info.computed_property_access(span) else {
            return self.is_property_read(span).then(|| {
                self.unsupported_operand(
                    "computed property without checked access classification".into(),
                    scope,
                    source_span,
                    out,
                )
            });
        };
        let name = access.property.clone();

        // ---- Checked getter target ----
        let Some(canonical) = self
            .type_info
            .resolved_identity(span)
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Property)
            .cloned()
        else {
            return Some(self.unsupported_operand(
                "computed property without checked getter identity".into(),
                scope,
                source_span,
                out,
            ));
        };
        // ---- Closed result and receiver ----
        let returned = match self.property_result_type(base, span, &canonical) {
            Ok(returned) => returned,
            Err(description) => return Some(self.unsupported_operand(description, scope, source_span, out)),
        };
        self.settled_types.insert((span.start, span.end), returned.clone());
        let receiver = self.lower_expr_to_place(base, scope, out);

        // ---- Getter call ----
        Some(self.push_call_temp(
            bir::Callee::Method(bir::MethodTarget {
                native_trait_delegation: None,
                name,
                canonical: Some(canonical),
                intrinsic_carrier: None,
                reflection: None,
                loaf_trait: None,
                trait_arguments: None,
                type_args: Vec::new(),
                binding: bir::ArgumentBinding::resolved_positional(0),
            }),
            fixed_elements(vec![bir::Operand::place(receiver, bir::OwnershipFact::Borrow, false)]),
            returned,
            scope,
            source_span,
            false,
            out,
        ))
    }

    /// Close a nominal getter's declared result from its checked owner frame, preserving trait-bound read facts.
    /// Only identical canonical nominal owners authorize substitution, including retained carrier keys backed by
    /// checked identities; absent local signatures retain the checked result.
    fn property_result_type(
        &self,
        base: &ast::Spanned<ast::Expr>,
        span: ast::Span,
        canonical: &CanonicalSymbolId,
    ) -> Result<IncanType, String> {
        let Some(binding) = self
            .type_info
            .declarations
            .body_ir_property_bindings
            .values()
            .find(|binding| binding.callable.identity.as_ref() == Some(canonical))
        else {
            return Ok(self.resolve_ty(span));
        };

        // ---- Declared owner ----
        let declared = parameter_types::checked_semantic_type(&binding.receiver_type, self.type_info);
        let declared_name = match &declared {
            IncanType::Named(name) => name,
            IncanType::Generic { base, .. } => base,
            _ => return Ok(self.resolve_ty(span)),
        };
        let Some(owner) = parameter_types::retained_nominal_identity(declared_name, self.type_info) else {
            return Err("computed property without canonical owner frame".into());
        };
        if owner.kind == SemanticSourceTargetKind::Trait {
            return Ok(self.resolve_ty(span));
        }

        // ---- Checked receiver frame ----
        let receiver = self.resolve_ty(base.span);
        let (name, arguments) = match &receiver {
            IncanType::Named(name) => (name, &[][..]),
            IncanType::Generic { base, args } => (base, args.as_slice()),
            _ => return Err("computed property without retained nominal receiver".into()),
        };
        if parameter_types::retained_nominal_identity(name, self.type_info) != Some(owner)
            || arguments.len() != binding.owner_parameters.len()
        {
            return Err("computed property owner differs from checked getter frame".into());
        }

        // ---- Closed result ----
        let parameters = binding
            .owner_parameters
            .iter()
            .cloned()
            .zip(arguments.iter().cloned())
            .collect();
        let returned = parameter_types::checked_semantic_type(&binding.callable.return_type, self.type_info);
        Ok(close_parameters(
            parameter_types::retain_parameter_type(returned, &binding.owner_parameters),
            &parameters,
        ))
    }
}

/// Substitute only binders declared by a checked getter owner, at every callable and container type position.
/// Unbound parameters remain explicit placeholders and therefore keep native admission fail-closed.
fn close_parameters(ty: IncanType, parameters: &HashMap<String, IncanType>) -> IncanType {
    match ty {
        IncanType::TypeVar(name) => parameters.get(&name).cloned().unwrap_or(IncanType::TypeVar(name)),
        IncanType::Generic { base, args } => IncanType::Generic {
            base,
            args: args.into_iter().map(|ty| close_parameters(ty, parameters)).collect(),
        },
        IncanType::Tuple(items) => {
            IncanType::Tuple(items.into_iter().map(|ty| close_parameters(ty, parameters)).collect())
        }
        IncanType::Ref(ty) => IncanType::Ref(Box::new(close_parameters(*ty, parameters))),
        IncanType::RefMut(ty) => IncanType::RefMut(Box::new(close_parameters(*ty, parameters))),
        IncanType::TypeToken(ty) => IncanType::TypeToken(Box::new(close_parameters(*ty, parameters))),
        IncanType::Function {
            mut params,
            return_type,
        } => {
            for parameter in &mut params {
                parameter.ty = close_parameters(parameter.ty.clone(), parameters);
            }
            IncanType::Function {
                params,
                return_type: Box::new(close_parameters(*return_type, parameters)),
            }
        }
        ty => ty,
    }
}
