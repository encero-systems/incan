//! Declaration-owned forwarding bodies for checked source Partial declarations.

use super::args::fixed_elements;
use super::{
    AbiV0RuntimeRequirement, BodyBuilder, BodyIrLoweringFacts, CanonicalSymbolId, CompilerNodeId,
    FunctionDefaultSource, HirSourceSpan, IncanType, SemanticSourceTargetKind, ast, bir, hir_span,
    semantic_type_from_resolved,
};
use crate::typechecker::PartialProjectionTargetKind;

/// The exact physically retained function a Partial forwards to, with its deferred source defaults.
struct PartialFunctionTarget {
    name: String,
    canonical: CanonicalSymbolId,
    direct_call_id: CompilerNodeId,
    defaults: Vec<FunctionDefaultSource>,
}

/// The physically retained model or class a constructor Partial builds, with its fields' deferred source defaults.
struct PartialConstructorTarget {
    declaration: bir::NominalDeclaration,
    defaults: Vec<FunctionDefaultSource>,
}

/// The checked target a source Partial forwards its complete parameter surface to.
enum PartialTarget {
    Function(PartialFunctionTarget),
    Constructor(PartialConstructorTarget),
}

impl PartialTarget {
    /// The target's deferred source defaults, one per forwarded parameter.
    fn defaults(&self) -> &[FunctionDefaultSource] {
        match self {
            Self::Function(target) => &target.defaults,
            Self::Constructor(target) => &target.defaults,
        }
    }
}

/// Select a Partial's target from the checker's recorded projection kind: a model or class constructor, or otherwise
/// a function.
fn partial_target(partial: &ast::PartialDecl, facts: &BodyIrLoweringFacts<'_, '_>) -> Result<PartialTarget, String> {
    match facts
        .type_info
        .partial_projection(&partial.name)
        .map(|projection| &projection.target_kind)
    {
        Some(PartialProjectionTargetKind::ModelConstructor | PartialProjectionTargetKind::ClassConstructor) => {
            constructor_target(partial, facts).map(PartialTarget::Constructor)
        }
        _ => function_target(partial, facts).map(PartialTarget::Function),
    }
}

/// Prove a constructor Partial's target is one retained model or class of this module, by its checked identity.
///
/// The forwarded surface is the declaration's complete field layout in order, so a field the checker projected in
/// another order, or a declaration without retained layout facts, keeps the Partial refused.
fn constructor_target(
    partial: &ast::PartialDecl,
    facts: &BodyIrLoweringFacts<'_, '_>,
) -> Result<PartialConstructorTarget, String> {
    let [name] = partial.target.segments.as_slice() else {
        return Err("source Partial with a nonlocal constructor target".to_string());
    };
    let canonical = facts
        .type_info
        .declarations
        .named_type_identities
        .get(name)
        .ok_or_else(|| "source Partial without a checked constructor target".to_string())?;
    let declaration = facts
        .local_nominal_declarations
        .values()
        .find(|declaration| &declaration.canonical == canonical)
        .ok_or_else(|| "source Partial over an unretained or nonlocal constructor".to_string())?;
    let defaults = facts
        .function_default_sources
        .get(name)
        .filter(|defaults| defaults.len() == declaration.fields.len())
        .ok_or_else(|| "source Partial without retained constructor field defaults".to_string())?;
    Ok(PartialConstructorTarget {
        declaration: declaration.clone(),
        defaults: defaults.clone(),
    })
}

/// Prove a Partial's target is one physically retained, checked function rather than resolving a backend spelling.
///
/// The physical declaration's span-keyed checked identity must match the selected binding in full. Package
/// publication gives that identity a package origin while this module still uses its source-module path; comparing
/// those path spellings would incorrectly refuse a physically local target.
fn function_target(
    partial: &ast::PartialDecl,
    facts: &BodyIrLoweringFacts<'_, '_>,
) -> Result<PartialFunctionTarget, String> {
    let [name] = partial.target.segments.as_slice() else {
        return Err("source Partial with a nonlocal target".to_string());
    };
    let binding = facts
        .type_info
        .declarations
        .function_bindings
        .get(name)
        .ok_or_else(|| "source Partial without a checked function target".to_string())?;
    let canonical = binding
        .identity
        .as_ref()
        .filter(|identity| identity.kind == SemanticSourceTargetKind::Function)
        .ok_or_else(|| "source Partial with a constructor, Partial, or nonlocal target".to_string())?;
    let name = &canonical.declaration_name;
    let spans = facts
        .local_function_declarations
        .get(name)
        .ok_or_else(|| "source Partial without an executable function declaration".to_string())?;
    let [span] = spans.as_slice() else {
        return Err("source Partial with an overloaded target".to_string());
    };
    let physically_owned = facts
        .type_info
        .declarations
        .function_bindings_by_span
        .get(&(span.start, span.end))
        .and_then(|binding| binding.identity.as_ref())
        == Some(canonical);
    if !physically_owned {
        return Err("source Partial without its checked physical target identity".to_string());
    }
    let defaults = facts
        .function_default_sources
        .get(name)
        .filter(|defaults| defaults.len() == binding.params.len())
        .ok_or_else(|| "source Partial without retained target defaults".to_string())?;
    Ok(PartialFunctionTarget {
        name: name.clone(),
        canonical: canonical.clone(),
        direct_call_id: CompilerNodeId::declaration_span(facts.module_identity, span.start, span.end),
        defaults: defaults.clone(),
    })
}

/// Keep every target parameter on the wrapper surface, replacing only its preset default computation.
fn partial_parameters(
    builder: &mut BodyBuilder<'_, '_>,
    binding: &crate::typechecker::FunctionBindingInfo,
    partial: &ast::PartialDecl,
    target: Option<&PartialTarget>,
    scope: bir::ScopeId,
    span: HirSourceSpan,
) -> Vec<bir::CallableParam> {
    let mut parameters = binding
        .params
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            let name = parameter
                .name
                .clone()
                .unwrap_or_else(|| format!("__partial_arg_{index}"));
            let ty = semantic_type_from_resolved(&parameter.ty);
            let local = builder.declare_new_local_with_reads(name.clone(), ty.clone(), scope, span, 1);
            builder.locals[local.index()].origin = bir::LocalOrigin::Parameter;
            if parameter.is_mut {
                builder.borrowed_parameters.insert(local);
            }
            bir::CallableParam {
                local,
                name,
                ty,
                span,
                default: bir::CallableParamDefault::Required,
                mutable: parameter.is_mut,
            }
        })
        .collect::<Vec<_>>();
    // Deferred defaults may introduce temporaries; every parameter must already occupy its declaration slot.
    for (index, parameter) in parameters.iter_mut().enumerate() {
        let source = partial
            .args
            .iter()
            .find(|preset| preset.name == parameter.name)
            .map(|preset| &preset.value)
            .or_else(|| {
                target
                    .and_then(|target| target.defaults().get(index))
                    .and_then(|source| source.default.as_ref())
            });
        parameter.default = if source.is_none() && binding.params[index].has_default {
            bir::CallableParamDefault::Unsupported {
                span,
                description: "source Partial parameter without its retained default".to_string(),
            }
        } else {
            builder.lower_callable_default(source, scope)
        };
    }
    parameters
}

/// Forward all declaration slots once, preserving the target's mutable passing contract and exact identity.
///
/// A function target is called; a constructor target builds its declaration from the slots, which must be its
/// complete field layout in order.
fn forward_partial(
    builder: &mut BodyBuilder<'_, '_>,
    target: PartialTarget,
    params: &[bir::CallableParam],
    scope: bir::ScopeId,
    span: HirSourceSpan,
    out: &mut Vec<bir::Statement>,
) {
    let operands = params
        .iter()
        .map(|parameter| {
            let place = bir::Place::from_local(parameter.local);
            let (fact, last_use) = builder.ownership_fact_for_place(&place, &parameter.ty);
            let operand = bir::Operand::place(place, fact, last_use);
            if parameter.mutable {
                builder.borrow_for_mut_parameter(operand)
            } else {
                operand
            }
        })
        .collect::<Vec<_>>();
    let ty = builder.owner_return_type.clone();
    let result = match target {
        PartialTarget::Function(target) => builder.push_call_temp(
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                receiver_type: None,
                name: target.name,
                direct_call_id: Some(target.direct_call_id),
                canonical: Some(target.canonical),
                builtin: None,
                type_args: Vec::new(),
                binding: bir::ArgumentBinding::resolved_positional(operands.len()),
            })),
            fixed_elements(operands),
            ty,
            scope,
            span,
            false,
            out,
        ),
        PartialTarget::Constructor(target) => {
            if !params
                .iter()
                .map(|parameter| &parameter.name)
                .eq(target.declaration.fields.iter())
            {
                builder.push_unsupported_stmt(
                    "source Partial whose parameters differ from its constructor's field layout".to_string(),
                    span,
                    out,
                );
                return;
            }
            let declaration = target.declaration;
            let construction = bir::ConstructorTarget {
                name: declaration.name,
                canonical: Some(declaration.canonical),
                direct_declaration_id: Some(declaration.direct_declaration_id),
                canonical_field_layout: Some(declaration.fields),
                binding: bir::ArgumentBinding::resolved_positional(operands.len()),
            };
            builder.push_assign_temp(
                bir::Rvalue::Aggregate(
                    bir::AggregateKind::Constructor(Box::new(construction)),
                    fixed_elements(operands),
                ),
                ty,
                scope,
                span,
                out,
            )
        }
    };
    out.push(bir::Statement {
        kind: bir::StatementKind::Return { value: Some(result) },
        span,
    });
    builder.insert_scope_drops(out, scope);
}

/// Retain a source Partial as an executable body, or a named unsupported body when target facts are unavailable.
/// Presets and residual source defaults execute only for omitted parameters, as in legacy's forwarding wrapper.
pub(super) fn lower_source_partial(
    partial: &ast::PartialDecl,
    decl_span: ast::Span,
    facts: &BodyIrLoweringFacts<'_, '_>,
) -> bir::Body {
    let span = hir_span(decl_span);
    let binding = facts
        .type_info
        .declarations
        .function_bindings_by_span
        .get(&(decl_span.start, decl_span.end));
    let return_type = binding
        .map(|binding| semantic_type_from_resolved(&binding.return_type))
        .unwrap_or(IncanType::Unknown);
    let mut builder = BodyBuilder::new(facts, return_type.clone());
    let scope = builder.new_scope(None, span);
    let target = partial_target(partial, facts);
    let params = binding
        .map(|binding| partial_parameters(&mut builder, binding, partial, target.as_ref().ok(), scope, span))
        .unwrap_or_default();
    let mut stmts = Vec::new();
    match target {
        Ok(target)
            if binding.is_some_and(|binding| {
                binding
                    .params
                    .iter()
                    .all(|parameter| parameter.name.is_some() && parameter.kind == ast::ParamKind::Normal)
            }) =>
        {
            forward_partial(&mut builder, target, &params, scope, span, &mut stmts);
        }
        Ok(_) => builder.push_unsupported_stmt(
            "source Partial without a complete checked parameter surface".to_string(),
            span,
            &mut stmts,
        ),
        Err(description) => builder.push_unsupported_stmt(description, span, &mut stmts),
    }
    if builder
        .locals
        .iter()
        .any(|local| !local.ty.abi_v0_facts().ownership.is_trivially_copy())
    {
        builder.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
    }
    let direct_call_id = CompilerNodeId::declaration_span(facts.module_identity, decl_span.start, decl_span.end);
    bir::Body {
        decl_id: direct_call_id.clone(),
        direct_call_id,
        canonical: binding.and_then(|binding| binding.identity.clone()),
        name: partial.name.clone(),
        type_parameters: Vec::new(),
        span,
        return_type,
        named_type_identities: facts.type_info.declarations.named_type_identities.clone(),
        param_locals: params.iter().map(|parameter| parameter.local).collect(),
        params,
        locals: builder.locals,
        scopes: builder.scopes,
        block: bir::Block { scope, stmts },
        runtime_requirements: builder.runtime_requirements,
        panic_facts: builder.panic_facts,
        is_async: false,
        extern_delegation: None,
        // No checked callable facts are recorded for a synthesized partial; callable signatures refuse by name.
        callable_representation: None,
    }
}
