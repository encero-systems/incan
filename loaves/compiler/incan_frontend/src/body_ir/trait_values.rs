//! Legacy trait inputs are hidden concrete parameters, never boxed trait objects.
//!
//! This projection changes only Body IR. Each independent input receives a binder with its canonical trait
//! identity, and each checked call retains the concrete carrier occupying that input's declaration slot.

use super::{CanonicalSymbolId, IncanType, SemanticSourceTargetKind, TypeCheckInfo, ast, bir};
use std::collections::{BTreeMap, HashMap};

mod inference;

/// One checked function's hidden input slots, appended after its explicitly declared type parameters.
struct TraitInputs {
    explicit_count: usize,
    parameters: Vec<String>,
    slots: Vec<usize>,
    annotations: Vec<(IncanType, CanonicalSymbolId)>,
}

/// Retain the generic representation of trait inputs and the concrete argument bindings that instantiate it.
/// Unknown carriers and unbound arguments stay open so native admission refuses rather than guessing a layout.
pub(super) fn retain_inputs(
    module: &mut bir::BodyIrModule,
    program: &ast::Program,
    facts: &TypeCheckInfo,
    context: &[bir::BodyIrModule],
) {
    let adoptions = super::collect::collect_trait_argument_witnesses(program, facts, &module.nominal_declarations);
    let mut signatures = HashMap::new();
    for body in &mut module.bodies {
        let Some(identity) = body.canonical.clone() else {
            continue;
        };
        if identity.kind != SemanticSourceTargetKind::Function {
            continue;
        }
        retain_annotation_identities(body, program, facts);
        let inputs = retain_body_inputs(body);
        if !inputs.slots.is_empty() {
            signatures.insert(identity, inputs);
        }
    }
    for body in context.iter().flat_map(|module| module.callable_declaration_frames()) {
        let slots = body
            .params
            .iter()
            .enumerate()
            .filter_map(|(slot, param)| {
                let IncanType::TypeVar(name) = &param.ty else {
                    return None;
                };
                body.named_type_identities
                    .get(name)
                    .filter(|identity| identity.kind == SemanticSourceTargetKind::Trait)
                    .map(|_| slot)
            })
            .collect::<Vec<_>>();
        if let Some(identity) = &body.canonical
            && !slots.is_empty()
        {
            signatures.insert(
                identity.clone(),
                TraitInputs {
                    explicit_count: body.type_parameters.len().saturating_sub(slots.len()),
                    parameters: body.type_parameters[..body.type_parameters.len().saturating_sub(slots.len())].to_vec(),
                    annotations: Vec::new(),
                    slots,
                },
            );
        }
    }
    for body in &mut module.bodies {
        retain_block_inputs(
            &mut body.block,
            &mut body.locals,
            &signatures,
            &module.nominal_declarations,
            &adoptions,
            &body.named_type_identities,
        );
    }
}

/// Retain identities from the checked annotation spans rather than widening legacy's nominal-only identity table.
fn retain_annotation_identities(body: &mut bir::Body, program: &ast::Program, facts: &TypeCheckInfo) {
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
    for (source, parameter) in function.params.iter().zip(&body.params) {
        let Some(identity) = facts
            .resolved_identity(source.node.ty.span)
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Trait)
        else {
            continue;
        };
        let name = match &parameter.ty {
            IncanType::Named(name) => name,
            IncanType::Generic { base, .. } => base,
            _ => continue,
        };
        body.named_type_identities.insert(name.clone(), identity.clone());
    }
}

/// Mint one distinct hidden binder for each canonically proven trait parameter, preserving parameter order.
fn retain_body_inputs(body: &mut bir::Body) -> TraitInputs {
    let explicit_count = body.type_parameters.len();
    let parameters = body.type_parameters.clone();
    let mut slots = Vec::new();
    let mut annotations = Vec::new();
    for (slot, parameter) in body.params.iter_mut().enumerate() {
        let name = match &parameter.ty {
            IncanType::Named(name) => name,
            IncanType::Generic { base, .. } => base,
            _ => continue,
        };
        let Some(identity) = body
            .named_type_identities
            .get(name)
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Trait)
            .cloned()
        else {
            continue;
        };
        let hidden = format!("__IncanTrait{}", slots.len());
        // A colliding explicit source binder cannot be silently replaced.
        if body.type_parameters.contains(&hidden) {
            continue;
        }
        annotations.push((parameter.ty.clone(), identity.clone()));
        body.type_parameters.push(hidden.clone());
        body.named_type_identities.insert(hidden.clone(), identity);
        parameter.ty = IncanType::TypeVar(hidden);
        body.locals[parameter.local.index()].ty = parameter.ty.clone();
        slots.push(slot);
    }
    TraitInputs {
        explicit_count,
        parameters,
        slots,
        annotations,
    }
}

/// Follow evaluation order and retained slot bindings through ordinary control-flow blocks.
fn retain_block_inputs(
    block: &mut bir::Block,
    locals: &mut [bir::LocalDecl],
    signatures: &HashMap<CanonicalSymbolId, TraitInputs>,
    nominals: &[bir::NominalDeclaration],
    adoptions: &BTreeMap<(CanonicalSymbolId, CanonicalSymbolId), Vec<IncanType>>,
    identities: &BTreeMap<String, CanonicalSymbolId>,
) {
    for statement in &mut block.stmts {
        match &mut statement.kind {
            bir::StatementKind::Assign { place, rvalue } if place.projection.is_empty() => {
                let Some(local) = place.local_id() else { continue };
                if !is_trait_type(&locals[local.index()].ty, identities) {
                    continue;
                }
                let carrier = match rvalue {
                    bir::Rvalue::Aggregate(bir::AggregateKind::Constructor(target), _) => target
                        .canonical
                        .as_ref()
                        .and_then(|identity| {
                            nominals
                                .iter()
                                .find(|nominal| &nominal.canonical == identity && nominal.type_parameters.is_empty())
                        })
                        .map(|nominal| IncanType::Named(nominal.name.clone())),
                    bir::Rvalue::Use(operand) => operand_carrier(operand, locals),
                    _ => None,
                };
                if let Some(carrier) = carrier {
                    locals[local.index()].ty = carrier;
                }
            }
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                args,
                ..
            } => retain_call_inputs(target, args, locals, signatures, nominals, adoptions, identities),
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                retain_block_inputs(then_block, locals, signatures, nominals, adoptions, identities);
                if let Some(block) = else_block {
                    retain_block_inputs(block, locals, signatures, nominals, adoptions, identities);
                }
            }
            bir::StatementKind::Loop { body } => {
                retain_block_inputs(body, locals, signatures, nominals, adoptions, identities)
            }
            _ => {}
        }
    }
}

/// Authorize this projection only for a named type whose checked identity is a trait, including instantiated traits.
fn is_trait_type(ty: &IncanType, identities: &BTreeMap<String, CanonicalSymbolId>) -> bool {
    let name = match ty {
        IncanType::Named(name) => name,
        IncanType::Generic { base, .. } => base,
        _ => return false,
    };
    identities
        .get(name)
        .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Trait)
}

/// Append carriers in declaration-slot order, independently of the written order of named arguments.
fn retain_call_inputs(
    target: &mut bir::NamedCallableTarget,
    args: &[bir::ArgumentElement],
    locals: &[bir::LocalDecl],
    signatures: &HashMap<CanonicalSymbolId, TraitInputs>,
    nominals: &[bir::NominalDeclaration],
    adoptions: &BTreeMap<(CanonicalSymbolId, CanonicalSymbolId), Vec<IncanType>>,
    identities: &BTreeMap<String, CanonicalSymbolId>,
) {
    let Some(inputs) = target.canonical.as_ref().and_then(|identity| signatures.get(identity)) else {
        return;
    };
    // A named call written out of declaration order keeps its declaration-order binding; carriers map by slot either
    // way.
    let Some(arguments) = target.binding.slot_arguments() else {
        return;
    };
    let carriers = inputs
        .slots
        .iter()
        .map(|slot| {
            let written = arguments.iter().position(|argument| argument.slot == *slot)?;
            let bir::ArgumentElement::One(operand) = args.get(written)? else {
                return None;
            };
            operand_carrier(operand, locals)
        })
        .collect::<Option<Vec<_>>>();
    if let Some(carriers) = carriers {
        if target.type_args.len() != inputs.explicit_count {
            if !target.type_args.is_empty() {
                return;
            }
            let Some(arguments) = inference::complete_arguments(inputs, &carriers, nominals, adoptions, identities)
            else {
                return;
            };
            target.type_args = arguments;
        }
        target.type_args.extend(carriers);
    }
}

/// Read a carrier already represented by an unprojected local; projection typing remains its owning boundary's job.
fn operand_carrier(operand: &bir::Operand, locals: &[bir::LocalDecl]) -> Option<IncanType> {
    let bir::Operand::Place(read) = operand else {
        return None;
    };
    if !read.place.projection.is_empty() {
        return None;
    }
    locals.get(read.place.local_id()?.index()).map(|local| local.ty.clone())
}
