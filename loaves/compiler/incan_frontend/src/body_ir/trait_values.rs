//! Legacy trait inputs are hidden concrete parameters, never boxed trait objects.
//!
//! This projection changes only Body IR. Each independent input receives a binder with its canonical trait
//! identity, and each checked call retains the concrete carrier occupying that input's declaration slot.

use super::{CanonicalSymbolId, IncanType, SemanticSourceTargetKind, bir};
use std::collections::{BTreeMap, HashMap};

/// One checked function's hidden input slots, appended after its explicitly declared type parameters.
struct TraitInputs {
    explicit_count: usize,
    slots: Vec<usize>,
}

/// Retain the generic representation of trait inputs and the concrete argument bindings that instantiate it.
/// Unknown carriers and unbound arguments stay open so native admission refuses rather than guessing a layout.
pub(super) fn retain_inputs(module: &mut bir::BodyIrModule, context: &[bir::BodyIrModule]) {
    let mut signatures = HashMap::new();
    for body in &mut module.bodies {
        let Some(identity) = body.canonical.clone() else {
            continue;
        };
        if identity.kind != SemanticSourceTargetKind::Function {
            continue;
        }
        let inputs = retain_body_inputs(body);
        if !inputs.slots.is_empty() {
            signatures.insert(identity, inputs);
        }
    }
    for body in context.iter().flat_map(|module| &module.bodies) {
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
            &body.named_type_identities,
        );
    }
}

/// Mint one distinct hidden binder for each canonically proven trait parameter, preserving parameter order.
fn retain_body_inputs(body: &mut bir::Body) -> TraitInputs {
    let explicit_count = body.type_parameters.len();
    let mut slots = Vec::new();
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
        body.type_parameters.push(hidden.clone());
        body.named_type_identities.insert(hidden.clone(), identity);
        parameter.ty = IncanType::TypeVar(hidden);
        body.locals[parameter.local.index()].ty = parameter.ty.clone();
        slots.push(slot);
    }
    TraitInputs { explicit_count, slots }
}

/// Follow evaluation order and retained slot bindings through ordinary control-flow blocks.
fn retain_block_inputs(
    block: &mut bir::Block,
    locals: &mut [bir::LocalDecl],
    signatures: &HashMap<CanonicalSymbolId, TraitInputs>,
    nominals: &[bir::NominalDeclaration],
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
            } => retain_call_inputs(target, args, locals, signatures),
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                retain_block_inputs(then_block, locals, signatures, nominals, identities);
                if let Some(block) = else_block {
                    retain_block_inputs(block, locals, signatures, nominals, identities);
                }
            }
            bir::StatementKind::Loop { body } => retain_block_inputs(body, locals, signatures, nominals, identities),
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
) {
    let Some(inputs) = target.canonical.as_ref().and_then(|identity| signatures.get(identity)) else {
        return;
    };
    if target.type_args.len() != inputs.explicit_count {
        return;
    }
    let bir::ArgumentBinding::Resolved { arguments, .. } = &target.binding else {
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
