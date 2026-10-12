//! Retain the concrete ABI of local opaque trait returns without changing legacy typecheck facts.

use super::{CanonicalSymbolId, IncanType, SemanticSourceTargetKind, bir};

/// One source function whose checked trait contract has a single proven nominal carrier.
struct Carrier {
    function: CanonicalSymbolId,
    declared: IncanType,
    concrete: IncanType,
    self_results: Vec<CanonicalSymbolId>,
}

/// Keep local opaque return carriers at the Body IR boundary; unresolved producers retain their original refusal.
///
/// Only nongeneric source functions with straight-line, unprojected nominal returns qualify. The checked source
/// establishes the trait obligation; this pass retains the already checked value type legacy uses for `impl Trait`.
/// It never resolves imported traits, selects an adopter by spelling, or creates a trait-object layout.
pub(super) fn retain(module: &mut bir::BodyIrModule) {
    let carriers = module
        .bodies
        .iter()
        .filter_map(|body| carrier(module, body))
        .collect::<Vec<_>>();
    for body in &mut module.bodies {
        if let Some(selected) = carriers
            .iter()
            .find(|item| body.canonical.as_ref() == Some(&item.function))
        {
            body.return_type = selected.concrete.clone();
        }
        retain_call_results(&mut body.block, &mut body.locals, &carriers);
    }
}

/// Prove one local trait-returning function's carrier from its checked return places, not from its interface name.
fn carrier(module: &bir::BodyIrModule, body: &bir::Body) -> Option<Carrier> {
    let function = body.canonical.as_ref()?;
    if function.kind != SemanticSourceTargetKind::Function || !body.type_parameters.is_empty() {
        return None;
    }
    let name = match &body.return_type {
        IncanType::Named(name) => name,
        IncanType::Generic { base, .. } => base,
        _ => return None,
    };
    let trait_owner = module
        .trait_declarations
        .iter()
        .find(|declaration| declaration.declaration_name == *name)?;
    let concrete = return_carrier(body)?;
    let IncanType::Named(owner) = &concrete else {
        return None;
    };
    if !module
        .nominal_declarations
        .iter()
        .any(|declaration| declaration.name == *owner && declaration.canonical.origin == function.origin)
    {
        return None;
    }
    Some(Carrier {
        function: function.clone(),
        declared: body.return_type.clone(),
        concrete,
        self_results: self_returning_methods(module, trait_owner),
    })
}

/// Read the local trait's physical default signatures whose checked result is exactly Self.
fn self_returning_methods(module: &bir::BodyIrModule, owner: &CanonicalSymbolId) -> Vec<CanonicalSymbolId> {
    module
        .bodies
        .iter()
        .filter_map(|body| {
            let identity = body.canonical.as_ref()?;
            (identity.kind == SemanticSourceTargetKind::Method
                && identity.origin == owner.origin
                && owner.declaration_span.start <= identity.declaration_span.start
                && identity.declaration_span.end <= owner.declaration_span.end
                && body.return_type == IncanType::SelfType
                && body.params.len() == 1
                && body.type_parameters.is_empty())
            .then(|| identity.clone())
        })
        .collect()
}

/// Require every flat return to name the same checked nominal value; control-flow and projected returns stay opaque.
fn return_carrier(body: &bir::Body) -> Option<IncanType> {
    let mut concrete = None;
    for statement in &body.block.stmts {
        match &statement.kind {
            bir::StatementKind::Return { value: Some(value) } => {
                let ty = operand_type(value, &body.locals)?;
                if !matches!(ty, IncanType::Named(_)) || concrete.as_ref().is_some_and(|previous| previous != &ty) {
                    return None;
                }
                concrete = Some(ty);
            }
            bir::StatementKind::Assign { .. }
            | bir::StatementKind::Call { .. }
            | bir::StatementKind::Drop { .. }
            | bir::StatementKind::Expr { .. } => {}
            _ => return None,
        }
    }
    concrete
}

/// Keep a proven opaque producer's carrier through its local trait's Self-return contract.
///
/// A synthesized target keeps its absent canonical identity and remains non-executable. This retains the checked
/// signature's ABI only: no body or dispatch permission is granted by the source spelling. Resolved targets must
/// match the retained physical slot exactly. Unknown receivers and trait parameters keep their original types.
fn retain_self_result(
    destination: &bir::Place,
    target: &bir::MethodTarget,
    args: &[bir::ArgumentElement],
    locals: &mut [bir::LocalDecl],
    carriers: &[Carrier],
) {
    if !destination.projection.is_empty() || !target.type_args.is_empty() {
        return;
    }
    let Some(bir::ArgumentElement::One(receiver)) = args.first() else {
        return;
    };
    let Some(receiver) = operand_type(receiver, locals) else {
        return;
    };
    let Some(carrier) = carriers.iter().find(|carrier| {
        carrier.concrete == receiver
            && carrier.self_results.iter().any(|slot| {
                slot.declaration_name == target.name
                    && target.canonical.as_ref().is_none_or(|identity| identity == slot)
            })
    }) else {
        return;
    };
    let Some(local) = destination
        .local_id()
        .and_then(|local| usize::try_from(local.0).ok())
        .and_then(|local| locals.get_mut(local))
    else {
        return;
    };
    if local.origin == bir::LocalOrigin::Temporary && local.ty == carrier.declared {
        local.ty = carrier.concrete.clone();
    }
}

/// Read only a whole local's existing checked type, refusing globals, projections, and literals.
fn operand_type(value: &bir::Operand, locals: &[bir::LocalDecl]) -> Option<IncanType> {
    let bir::Operand::Place(read) = value else {
        return None;
    };
    if !read.place.projection.is_empty() {
        return None;
    }
    let local = read.place.local_id()?;
    locals.get(usize::try_from(local.0).ok()?).map(|local| local.ty.clone())
}

/// Carry one canonical function's proven return through call destinations, including calls nested in control flow.
fn retain_call_results(block: &mut bir::Block, locals: &mut [bir::LocalDecl], carriers: &[Carrier]) {
    for statement in &mut block.stmts {
        match &mut statement.kind {
            bir::StatementKind::Call {
                destination: Some(destination),
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } => {
                if !destination.projection.is_empty() || !target.type_args.is_empty() {
                    continue;
                }
                let Some(carrier) = carriers
                    .iter()
                    .find(|item| target.canonical.as_ref() == Some(&item.function))
                else {
                    continue;
                };
                let Some(local) = destination
                    .local_id()
                    .and_then(|local| usize::try_from(local.0).ok())
                    .and_then(|local| locals.get_mut(local))
                else {
                    continue;
                };
                if local.origin == bir::LocalOrigin::Temporary && local.ty == carrier.declared {
                    local.ty = carrier.concrete.clone();
                }
            }
            bir::StatementKind::Call {
                destination: Some(destination),
                callee: bir::Callee::Method(target),
                args,
                ..
            } => retain_self_result(destination, target, args, locals, carriers),
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                retain_call_results(then_block, locals, carriers);
                if let Some(else_block) = else_block {
                    retain_call_results(else_block, locals, carriers);
                }
            }
            bir::StatementKind::Loop { body } => retain_call_results(body, locals, carriers),
            _ => {}
        }
    }
}
