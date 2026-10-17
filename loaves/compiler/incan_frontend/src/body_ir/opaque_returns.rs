//! Retain the concrete ABI of local opaque trait returns without changing legacy typecheck facts.

use super::{CanonicalSymbolId, IncanType, ResolvedType, SemanticSourceTargetKind, TypeCheckInfo, bir};

/// One source function whose checked trait contract has a single proven nominal carrier.
pub(super) struct Carrier {
    function: CanonicalSymbolId,
    declared: IncanType,
    concrete: IncanType,
    self_results: Vec<CanonicalSymbolId>,
}

/// Snapshot checked local opaque return contracts before representation refinement replaces their declared traits with
/// concrete carriers.
///
/// Only nongeneric source functions with straight-line, unprojected nominal returns qualify. The checked source
/// establishes the trait obligation; this pass retains the already checked value type legacy uses for `impl Trait`.
/// It never resolves imported traits, selects an adopter by spelling, or creates a trait-object layout.
pub(super) fn collect(module: &bir::BodyIrModule, type_info: &TypeCheckInfo) -> Vec<Carrier> {
    module
        .bodies
        .iter()
        .filter_map(|body| carrier(module, body, type_info))
        .collect()
}

/// Apply the checked opaque contracts captured before trait-return refinement replaces their interface carriers. The
/// snapshot preserves exact Self-returning slots without resolving names or adding dispatch authority.
pub(super) fn retain(module: &mut bir::BodyIrModule, carriers: &[Carrier]) {
    for body in &mut module.bodies {
        if let Some(selected) = carriers
            .iter()
            .find(|item| body.canonical.as_ref() == Some(&item.function))
        {
            body.return_type = selected.concrete.clone();
        }
        retain_call_results(&mut body.block, &mut body.locals, carriers);
    }
}

/// Prove one local trait-returning function's carrier from its checked return places, not from its interface name.
fn carrier(module: &bir::BodyIrModule, body: &bir::Body, type_info: &TypeCheckInfo) -> Option<Carrier> {
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
    let selected = super::parameter_types::retained_nominal_identity(owner, type_info)?;
    if !module
        .nominal_declarations
        .iter()
        .any(|declaration| &declaration.canonical == selected && declaration.canonical.origin == function.origin)
    {
        return None;
    }
    Some(Carrier {
        function: function.clone(),
        declared: body.return_type.clone(),
        concrete,
        self_results: self_returning_methods(module, trait_owner, type_info),
    })
}

/// Read a local trait's physical defaults with a checked Self result and only its owner's generic binders.
/// Owner parameters are retained by generic trait bodies; extra method binders and explicit trait returns grant no
/// Self-carrier relation.
fn self_returning_methods(
    module: &bir::BodyIrModule,
    owner: &CanonicalSymbolId,
    type_info: &TypeCheckInfo,
) -> Vec<CanonicalSymbolId> {
    module
        .bodies
        .iter()
        .filter_map(|body| {
            let identity = body.canonical.as_ref()?;
            let binding = type_info
                .declarations
                .method_bindings_by_span
                .get(&(identity.declaration_span.start, identity.declaration_span.end))?;
            (identity.kind == SemanticSourceTargetKind::Method
                && identity.origin == owner.origin
                && owner.declaration_span.start <= identity.declaration_span.start
                && identity.declaration_span.end <= owner.declaration_span.end
                && binding.identity.as_ref() == Some(identity)
                && binding.return_type == ResolvedType::SelfType
                && body.params.len() == 1
                && body.type_parameters.as_slice()
                    == type_info
                        .traits
                        .type_params
                        .get(&owner.declaration_name)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]))
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

/// Keep a checked alias of a canonical producer's result in the same concrete carrier, preserving its ownership fact.
fn retain_assignment_result(
    destination: &bir::Place,
    source: &bir::Operand,
    locals: &mut [bir::LocalDecl],
    carriers: &[Carrier],
) {
    if !destination.projection.is_empty() {
        return;
    }
    let Some(source_type) = operand_type(source, locals) else {
        return;
    };
    let Some(local) = destination.local_id().and_then(|id| locals.get_mut(id.index())) else {
        return;
    };
    if carriers
        .iter()
        .any(|carrier| carrier.concrete == source_type && carrier.declared == local.ty)
    {
        local.ty = source_type;
    }
}

/// Carry one canonical function's proven return through call destinations and aliases, including nested control flow.
fn retain_call_results(block: &mut bir::Block, locals: &mut [bir::LocalDecl], carriers: &[Carrier]) {
    for statement in &mut block.stmts {
        match &mut statement.kind {
            bir::StatementKind::Assign {
                place,
                rvalue: bir::Rvalue::Use(source),
            } => retain_assignment_result(place, source, locals, carriers),
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
