//! Preserve the concrete carrier behind legacy's opaque trait results and supertrait returns.
//!
//! Only checked trait annotations, canonical callees, and a selected method's exact Self-result fact authorize
//! refinement. Different return carriers remain open and are refused; no boxing or vtable conversion is introduced.

use super::{CanonicalSymbolId, IncanType, SemanticSourceTargetKind, TypeCheckInfo, ast, bir};
use std::collections::{BTreeMap, HashMap};

/// A retained callable's original source result and its current concrete representation witness.
#[derive(Clone)]
struct ReturnSignature {
    original: IncanType,
    carrier: IncanType,
    parameters: Vec<String>,
    identities: BTreeMap<String, CanonicalSymbolId>,
    opaque: bool,
    hidden_inputs: Vec<(usize, usize)>,
}

/// Refine opaque frames and checked call results to one concrete witness, propagating through canonical calls.
/// The finite iteration bound follows the number of source bodies; unresolved cycles retain their named refusal.
pub(super) fn retain_returns(
    module: &mut bir::BodyIrModule,
    program: &ast::Program,
    facts: &TypeCheckInfo,
    context: &[bir::BodyIrModule],
) {
    let mut signatures = HashMap::new();
    for body in &mut module.bodies {
        retain_return_annotation(body, program, facts);
        if let Some(identity) = &body.canonical {
            signatures.insert(identity.clone(), signature(body));
        }
    }
    for body in context.iter().flat_map(|module| &module.bodies) {
        if let Some(identity) = &body.canonical {
            signatures.entry(identity.clone()).or_insert_with(|| signature(body));
        }
    }
    let original_locals = module
        .bodies
        .iter()
        .map(|body| body.locals.iter().map(|local| local.ty.clone()).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    for _ in 0..=module.bodies.len() {
        let mut changed = false;
        for (body, original) in module.bodies.iter_mut().zip(&original_locals) {
            refine_block(
                &mut body.block,
                &mut body.locals,
                &body.named_type_identities,
                &module.nominal_declarations,
                &signatures,
                facts,
                original,
            );
            let Some(identity) = &body.canonical else { continue };
            let Some(retained) = signatures.get_mut(identity) else {
                continue;
            };
            if !retained.opaque {
                continue;
            }
            if let Some(carrier) = return_witness(&body.block, &body.locals, &body.named_type_identities) {
                if carrier != body.return_type {
                    body.return_type = carrier.clone();
                    retained.carrier = carrier;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// Keep the original trait's exact annotation identity beside its executable witness, without changing checker facts.
fn retain_return_annotation(body: &mut bir::Body, program: &ast::Program, facts: &TypeCheckInfo) {
    let Some(function) = program.declarations.iter().find_map(|declaration| {
        if declaration.span.start != body.span.start || declaration.span.end != body.span.end {
            return None;
        }
        match &declaration.node {
            ast::Declaration::Function(function) => Some(function),
            _ => None,
        }
    }) else {
        return;
    };
    let Some(identity) = facts
        .resolved_identity(function.return_type.span)
        .filter(|identity| identity.kind == SemanticSourceTargetKind::Trait)
    else {
        return;
    };
    let Some(name) = type_base(&body.return_type) else {
        return;
    };
    body.named_type_identities.insert(name.to_owned(), identity.clone());
}

/// Snapshot the checked callable frame before refinement so only its declared result authorizes an opaque return.
fn signature(body: &bir::Body) -> ReturnSignature {
    ReturnSignature {
        original: body.return_type.clone(),
        carrier: body.return_type.clone(),
        parameters: body.type_parameters.clone(),
        identities: body.named_type_identities.clone(),
        opaque: trait_type(&body.return_type, &body.named_type_identities),
        hidden_inputs: body
            .params
            .iter()
            .enumerate()
            .filter_map(|(slot, parameter)| {
                let IncanType::TypeVar(name) = &parameter.ty else {
                    return None;
                };
                if !name.starts_with("__IncanTrait")
                    || !body
                        .named_type_identities
                        .get(name)
                        .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Trait)
                {
                    return None;
                }
                Some((
                    slot,
                    body.type_parameters.iter().position(|parameter| parameter == name)?,
                ))
            })
            .collect(),
    }
}

/// Follow checked carriers through ordinary statements and control flow, leaving projected or unknown values open.
fn refine_block(
    block: &mut bir::Block,
    locals: &mut [bir::LocalDecl],
    identities: &BTreeMap<String, CanonicalSymbolId>,
    nominals: &[bir::NominalDeclaration],
    signatures: &HashMap<CanonicalSymbolId, ReturnSignature>,
    facts: &TypeCheckInfo,
    original: &[IncanType],
) {
    for statement in &mut block.stmts {
        match &mut statement.kind {
            bir::StatementKind::Assign { place, rvalue } if place.projection.is_empty() => {
                let Some(local) = place.local_id() else { continue };
                let retained_copy = match rvalue {
                    bir::Rvalue::Use(bir::Operand::Place(read)) if read.place.projection.is_empty() => {
                        read.place.local_id().is_some_and(|source| {
                            original[source.index()] == original[local.index()]
                                && locals[source.index()].ty != original[source.index()]
                        })
                    }
                    _ => false,
                };
                if !trait_type(&original[local.index()], identities) && !retained_copy {
                    continue;
                }
                let carrier = match rvalue {
                    bir::Rvalue::Use(operand) => operand_type(operand, locals),
                    bir::Rvalue::Aggregate(bir::AggregateKind::Constructor(target), _) => target
                        .canonical
                        .as_ref()
                        .and_then(|identity| {
                            nominals
                                .iter()
                                .find(|nominal| &nominal.canonical == identity && nominal.type_parameters.is_empty())
                        })
                        .map(|nominal| IncanType::Named(nominal.name.clone())),
                    _ => None,
                };
                if let Some(carrier) = carrier {
                    locals[local.index()].ty = carrier;
                }
            }
            bir::StatementKind::Call {
                destination: Some(place),
                callee,
                args,
                ..
            } if place.projection.is_empty() => {
                let Some(local) = place.local_id() else { continue };
                if let bir::Callee::Function(bir::CallableTarget::Named(target)) = callee {
                    refresh_hidden_arguments(target, args, locals, signatures);
                }
                let carrier = call_carrier(callee, args, statement.span, locals, signatures, facts);
                let may_refine = trait_type(&locals[local.index()].ty, identities)
                    || matches!(callee, bir::Callee::Function(bir::CallableTarget::Named(target))
                        if target.canonical.as_ref().and_then(|id| signatures.get(id))
                            .is_some_and(|signature| original[local.index()] == signature.original
                                || trait_type(&original[local.index()], &signature.identities)));
                if may_refine && let Some(carrier) = carrier {
                    locals[local.index()].ty = carrier;
                }
            }
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                refine_block(then_block, locals, identities, nominals, signatures, facts, original);
                if let Some(block) = else_block {
                    refine_block(block, locals, identities, nominals, signatures, facts, original);
                }
            }
            bir::StatementKind::Loop { body } => {
                refine_block(body, locals, identities, nominals, signatures, facts, original)
            }
            _ => {}
        }
    }
}

/// Refresh only hidden trait input bindings from their represented declaration slots after return refinement.
/// Explicit type arguments remain the checker's original selection; an unresolved or projected argument stays open.
fn refresh_hidden_arguments(
    target: &mut bir::NamedCallableTarget,
    args: &[bir::ArgumentElement],
    locals: &[bir::LocalDecl],
    signatures: &HashMap<CanonicalSymbolId, ReturnSignature>,
) {
    let Some(signature) = target.canonical.as_ref().and_then(|identity| signatures.get(identity)) else {
        return;
    };
    if target.type_args.len() != signature.parameters.len() {
        return;
    }
    let bir::ArgumentBinding::Resolved { arguments, .. } = &target.binding else {
        return;
    };
    for (slot, parameter) in &signature.hidden_inputs {
        let Some(position) = arguments.iter().position(|argument| argument.slot == *slot) else {
            continue;
        };
        let Some(bir::ArgumentElement::One(operand)) = args.get(position) else {
            continue;
        };
        if let Some(carrier) = operand_type(operand, locals) {
            target.type_args[*parameter] = carrier;
        }
    }
}

/// Select a call's representation from the exact body or the checker's exact Self-result declaration fact.
fn call_carrier(
    callee: &bir::Callee,
    args: &[bir::ArgumentElement],
    span: super::HirSourceSpan,
    locals: &[bir::LocalDecl],
    signatures: &HashMap<CanonicalSymbolId, ReturnSignature>,
    facts: &TypeCheckInfo,
) -> Option<IncanType> {
    match callee {
        bir::Callee::Function(bir::CallableTarget::Named(target)) => {
            let signature = signatures.get(target.canonical.as_ref()?)?;
            substitute(&signature.carrier, &signature.parameters, &target.type_args)
        }
        bir::Callee::Method(target) => {
            let selected = facts.calls.body_ir_self_result_methods.get(&(span.start, span.end))?;
            if target.canonical.as_ref() != Some(selected) {
                return None;
            }
            let bir::ArgumentElement::One(receiver) = args.first()? else {
                return None;
            };
            operand_type(receiver, locals)
        }
        _ => None,
    }
}

/// Require every reachable retained return to agree on one non-trait carrier; mixed or unresolved results refuse.
fn return_witness(
    block: &bir::Block,
    locals: &[bir::LocalDecl],
    identities: &BTreeMap<String, CanonicalSymbolId>,
) -> Option<IncanType> {
    let mut returns = Vec::new();
    collect_returns(block, &mut returns);
    let mut witness = None;
    for value in returns {
        let carrier = operand_type(value?, locals)?;
        if trait_type(&carrier, identities) || carrier == IncanType::Unknown {
            return None;
        }
        if witness.as_ref().is_some_and(|known| known != &carrier) {
            return None;
        }
        witness = Some(carrier);
    }
    witness
}

/// Collect value returns from represented blocks without treating loop breaks or closure returns as function returns.
fn collect_returns<'a>(block: &'a bir::Block, out: &mut Vec<Option<&'a bir::Operand>>) {
    for statement in &block.stmts {
        match &statement.kind {
            bir::StatementKind::Return { value } => out.push(value.as_ref()),
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                collect_returns(then_block, out);
                if let Some(block) = else_block {
                    collect_returns(block, out);
                }
            }
            bir::StatementKind::Loop { body } => collect_returns(body, out),
            _ => {}
        }
    }
}

/// Read a represented unprojected local carrier; field, index, and other projection contracts remain untouched.
fn operand_type(operand: &bir::Operand, locals: &[bir::LocalDecl]) -> Option<IncanType> {
    let bir::Operand::Place(read) = operand else {
        return None;
    };
    if !read.place.projection.is_empty() {
        return None;
    }
    locals.get(read.place.local_id()?.index()).map(|local| local.ty.clone())
}

/// Close only placeholders declared by the selected body, preserving nested generic and tuple carriers.
fn substitute(ty: &IncanType, parameters: &[String], arguments: &[IncanType]) -> Option<IncanType> {
    match ty {
        IncanType::TypeVar(name) => arguments
            .get(parameters.iter().position(|parameter| parameter == name)?)
            .cloned(),
        IncanType::Generic { base, args } => Some(IncanType::Generic {
            base: base.clone(),
            args: args
                .iter()
                .map(|ty| substitute(ty, parameters, arguments))
                .collect::<Option<_>>()?,
        }),
        IncanType::Tuple(elements) => Some(IncanType::Tuple(
            elements
                .iter()
                .map(|ty| substitute(ty, parameters, arguments))
                .collect::<Option<_>>()?,
        )),
        ty => Some(ty.clone()),
    }
}

/// Whether a named carrier position has a checked trait identity, rather than an ordinary nominal layout.
fn trait_type(ty: &IncanType, identities: &BTreeMap<String, CanonicalSymbolId>) -> bool {
    type_base(ty)
        .and_then(|name| identities.get(name))
        .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Trait)
}

/// The checked named or generic base whose binding supplies one canonical trait identity.
fn type_base(ty: &IncanType) -> Option<&str> {
    match ty {
        IncanType::Named(name) => Some(name),
        IncanType::Generic { base, .. } => Some(base),
        _ => None,
    }
}
