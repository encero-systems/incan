//! Relocate straight-line frames of primitive values only when their full historical physical-input digest matches.

use std::collections::BTreeMap;

use super::{BodyLocal, CanonicalSymbolId, Fields, Positions, Span};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_semantics_core::body_ir as ir;
use incan_semantics_core::{
    CompilerNodeId, IncanCallableParamKind, IncanInputAccess, IncanType, SemanticSourceTargetKind, SymbolNamespace,
    SymbolOrigin, canonical_module_identity,
};

/// Each historical range is paired with the same structured syntax path in this invocation.
pub(super) struct Placement {
    previous_nodes: BTreeMap<String, Span>,
    current: BTreeMap<Span, Span>,
    previous: BTreeMap<Span, Span>,
    historical_targets: BTreeMap<CanonicalSymbolId, CanonicalSymbolId>,
}

impl Placement {
    /// Pair the prior ranges with the current structural paths, rejecting ambiguous or expanded coordinates.
    fn new(positions: &Positions, coordinates: &[(u32, u32)]) -> Option<Self> {
        let paths = positions.nodes.physical_nodes();
        if paths.len() != coordinates.len() || coordinates.is_empty() {
            return None;
        }
        let mut result = Self {
            previous_nodes: BTreeMap::new(),
            current: BTreeMap::new(),
            previous: BTreeMap::new(),
            historical_targets: BTreeMap::new(),
        };
        for ((path, &current), &(start, end)) in paths.iter().zip(coordinates) {
            if current.expansion.is_some() {
                return None;
            }
            let previous = Span::new(start as usize, end as usize);
            if result
                .current
                .insert(previous, current)
                .is_some_and(|was| was != current)
                || result
                    .previous
                    .insert(current, previous)
                    .is_some_and(|was| was != previous)
            {
                return None;
            }
            result.previous_nodes.insert(path.clone(), previous);
        }
        Some(result)
    }

    /// Return the prior ranges keyed by the syntax path used in the original physical digest.
    pub(super) fn nodes(&self) -> &BTreeMap<String, Span> {
        &self.previous_nodes
    }

    /// Look up the historical source range of a current checked node.
    pub(super) fn prior_span(&self, current: Span) -> Option<Span> {
        self.previous.get(&current).copied()
    }

    /// Project a resolved declaration back to the canonical identity used in the original physical digest.
    pub(super) fn prior_target<'a>(&'a self, current: &'a CanonicalSymbolId) -> &'a CanonicalSymbolId {
        self.historical_targets.get(current).unwrap_or(current)
    }

    /// Move one historical coordinate only when its current source range is witnessed.
    fn span(&self, span: &mut Span) -> Option<()> {
        *span = *self.current.get(span)?;
        Some(())
    }

    /// Replace the declaration range after checking all other canonical identity fields.
    fn symbol(&mut self, previous: &mut CanonicalSymbolId, current: &CanonicalSymbolId) -> Option<()> {
        let mut historical = current.clone();
        historical.declaration_span = previous.declaration_span;
        if historical != *previous
            || previous.declaration_span.expansion.is_some()
            || current.declaration_span.expansion.is_some()
        {
            return None;
        }
        if previous != current {
            if self
                .historical_targets
                .insert(current.clone(), historical.clone())
                .is_some_and(|other| other != historical)
            {
                return None;
            }
            *previous = current.clone();
        }
        Some(())
    }
}

/// Both variants serialize as the original ordered map. The current proof borrows the checked map directly.
#[derive(serde::Serialize)]
#[serde(untagged)]
pub(super) enum NestedBindings<'a> {
    Current(&'a BTreeMap<Span, CanonicalSymbolId>),
    Historical(BTreeMap<Span, &'a CanonicalSymbolId>),
}

/// Project local declaration coordinates and targets through the same map as the historical reads and writes.
pub(super) fn nested_bindings<'a>(
    local: &'a BodyLocal,
    placement: Option<&'a Placement>,
) -> Option<NestedBindings<'a>> {
    let Some(layout) = placement else {
        return Some(NestedBindings::Current(&local.nested));
    };
    let historical = local
        .nested
        .iter()
        .map(|(span, identity)| Some((layout.prior_span(*span)?, layout.prior_target(identity))))
        .collect::<Option<_>>()?;
    Some(NestedBindings::Historical(historical))
}

/// Rebuild a direct-call id only when the old module and declaration span reproduce it exactly.
fn direct_id(id: &mut CompilerNodeId, old: &CanonicalSymbolId, current: &CanonicalSymbolId) -> Option<()> {
    let module = canonical_module_identity(current)?;
    if *id != CompilerNodeId::declaration_at(&module, old.declaration_span) {
        return None;
    }
    *id = CompilerNodeId::declaration_at(&module, current.declaration_span);
    Some(())
}

/// A scalar local without a field, index or dereference contains no nested physical source references.
fn plain_place(place: &ir::Place) -> bool {
    matches!(place.root, ir::PlaceRoot::Local(_)) && place.projection.is_empty()
}

/// Identify operands whose complete physical input is carried by their statement and local.
fn plain_operand(op: &ir::Operand) -> bool {
    match op {
        ir::Operand::Place(read) => plain_place(&read.place),
        ir::Operand::Constant(value) => matches!(
            value,
            ir::Constant::Int(_)
                | ir::Constant::Float(_)
                | ir::Constant::Bool(_)
                | ir::Constant::Str(_)
                | ir::Constant::FrozenStr(_)
                | ir::Constant::Bytes(_)
                | ir::Constant::Unit
                | ir::Constant::None
        ),
    }
}

/// A parameter must be a required ordinary primitive in its producer's corresponding frame slot.
fn plain_parameter(body: &ir::Body, slot: usize, parameter: &ir::CallableParam) -> bool {
    let Some(local) = body.locals.get(slot) else {
        return false;
    };
    let Some(identity) = local.identity.as_deref() else {
        return false;
    };
    body.param_locals.get(slot) == Some(&parameter.local)
        && parameter.local.index() == slot
        && local.id == parameter.local
        && local.origin == ir::LocalOrigin::Parameter
        && local.scope == body.block.scope
        && local.name.as_deref() == Some(parameter.name.as_str())
        && local.span == parameter.span
        && local.ty == parameter.ty
        && matches!(parameter.ty, IncanType::Primitive(_))
        && parameter.kind == IncanCallableParamKind::Normal
        && matches!(parameter.default, ir::CallableParamDefault::Required)
        && !parameter.mutable
        && !parameter.native_owned
        && parameter.input_access == IncanInputAccess::Inferred
        && identity.kind == SemanticSourceTargetKind::Parameter
        && identity.namespace == SymbolNamespace::OrdinaryLexical
        && identity.declaration_name == parameter.name
        && identity.declaration_span == parameter.span
}

/// Admit only a built-in list with one primitive element type and no nested or borrowed storage type.
fn primitive_list(ty: &IncanType) -> bool {
    matches!(
        ty.collection_arguments(CollectionTypeId::List),
        Some([IncanType::Primitive(_)])
    )
}

/// Resolve an unprojected local to its declared placement slot.
fn placement_local<'a>(body: &'a ir::Body, place: &ir::Place) -> Option<&'a ir::LocalDecl> {
    if !plain_place(place) {
        return None;
    }
    body.locals.get(place.local_id()?.index())
}

/// Preserve primitive argument behavior while requiring fixed list arguments to have known, nonexclusive access.
fn placement_call_operand(body: &ir::Body, operand: &ir::Operand) -> bool {
    if let ir::Operand::Place(read) = operand
        && let Some(local) = placement_local(body, &read.place)
        && primitive_list(&local.ty)
    {
        return !matches!(read.fact, ir::OwnershipFact::MutBorrow | ir::OwnershipFact::Unknown);
    }
    plain_operand(operand)
}

/// Select primitive operands with no projections or exclusive loans for newly admitted rvalues.
fn primitive_operand(body: &ir::Body, operand: &ir::Operand) -> bool {
    match operand {
        ir::Operand::Constant(_) => plain_operand(operand),
        ir::Operand::Place(read) => {
            !matches!(read.fact, ir::OwnershipFact::MutBorrow | ir::OwnershipFact::Unknown)
                && placement_local(body, &read.place).is_some_and(|local| matches!(local.ty, IncanType::Primitive(_)))
        }
    }
}

/// Keep assignments within unprojected compiler temporaries whose allowed rvalues carry no extra source coordinates.
fn plain_assignment(body: &ir::Body, place: &ir::Place, value: &ir::Rvalue) -> bool {
    let Some(local) = placement_local(body, place) else {
        return false;
    };
    if local.origin != ir::LocalOrigin::Temporary {
        return false;
    }
    match value {
        ir::Rvalue::BinaryOp(ir::BinOp::Add, left, right) => {
            matches!(local.ty, IncanType::Primitive(_))
                && primitive_operand(body, left)
                && primitive_operand(body, right)
        }
        ir::Rvalue::Aggregate(ir::AggregateKind::List, items) => {
            primitive_list(&local.ty)
                && items
                    .iter()
                    .all(|item| matches!(item, ir::ArgumentElement::One(element) if primitive_operand(body, element)))
        }
        _ => false,
    }
}

/// Offer coordinates for straight-line primitive values and fixed lists passed through checked direct calls.
/// Other executable forms retain the unmodified physical proof and can only be reused at their original placement.
pub fn scalar_frame(body: &ir::Body) -> bool {
    body.type_parameters.is_empty()
        && body.nominal_instantiation.is_empty()
        && body.trait_default_instantiation.is_none()
        && body.native_attributes.is_empty()
        && body.named_type_identities.is_empty()
        && body.checked_input_profile.is_none()
        && body.explicit_input_bindings.is_empty()
        && body.returned_view.is_none()
        && !body.is_async
        && body.panic_facts.is_empty()
        && body.retired_rust_facade_wire.is_none()
        && matches!(body.return_type, IncanType::Primitive(_))
        && body.params.len() == body.param_locals.len()
        && body
            .params
            .iter()
            .enumerate()
            .all(|(slot, parameter)| plain_parameter(body, slot, parameter))
        && body.locals.iter().skip(body.params.len()).all(|local| {
            local.origin != ir::LocalOrigin::Parameter
                && local.identity.is_none()
                && (matches!(local.ty, IncanType::Primitive(_))
                    || (local.origin == ir::LocalOrigin::Temporary && primitive_list(&local.ty)))
        })
        && body.block.stmts.iter().all(|statement| match &statement.kind {
            ir::StatementKind::Return { value } => value.as_ref().is_none_or(plain_operand),
            ir::StatementKind::Drop { .. } => true,
            ir::StatementKind::Expr { value } => plain_operand(value),
            ir::StatementKind::Assign { place, rvalue } => plain_assignment(body, place, rvalue),
            ir::StatementKind::Call {
                destination,
                callee: ir::Callee::Function(ir::CallableTarget::Named(call)),
                args,
                ..
            } => {
                destination.as_ref().is_none_or(|place| {
                    placement_local(body, place).is_some_and(|local| matches!(local.ty, IncanType::Primitive(_)))
                }) && call.receiver_type.is_none()
                    && (call.type_args.is_empty()
                        || call.direct_call_id.is_some()
                            && call.builtin.is_none()
                            && call.type_args.iter().all(|ty| matches!(ty, IncanType::Primitive(_))))
                    && call.canonical.is_some()
                    && args
                        .iter()
                        .all(|arg| matches!(arg, ir::ArgumentElement::One(op) if placement_call_operand(body, op)))
            }
            _ => false,
        })
}

/// Rebind the parameter and local through the settled write fact used by fresh Body IR construction.
/// The declaration owns that fact even when the independently selected statement record omits it.
fn relocate_parameters(
    layout: &mut Placement,
    facts: &crate::typechecker::TypeCheckInfo,
    body: &mut ir::Body,
) -> Option<()> {
    for parameter in &mut body.params {
        layout.span(&mut parameter.span)?;
        let current = facts.resolved_write_identity(parameter.span, &parameter.name)?;
        if current.declaration_span != parameter.span {
            return None;
        }
        let previous = body.locals.get_mut(parameter.local.index())?.identity.as_deref_mut()?;
        layout.symbol(previous, current)?;
    }
    Some(())
}

/// Restore an eligible native frame and authenticate its historical coordinates against the full input digest.
pub(super) fn relocate(
    positions: &Positions,
    facts: &crate::typechecker::TypeCheckInfo,
    local: &BodyLocal,
    records: [&Fields; 3],
    expected: [u8; 32],
    coordinates: &[(u32, u32)],
    body: &mut ir::Body,
) -> Option<()> {
    if !scalar_frame(body) {
        return None;
    }
    let mut layout = Placement::new(positions, coordinates)?;
    let current = positions.identities.get(&positions.nodes.owner().render_compact())?;
    let previous = body.canonical.as_mut()?;
    if previous.kind != SemanticSourceTargetKind::Function {
        return None;
    }
    direct_id(&mut body.decl_id, previous, current)?;
    direct_id(&mut body.direct_call_id, previous, current)?;
    layout.symbol(previous, current)?;
    layout.span(&mut body.span)?;
    if body.span != positions.owner {
        return None;
    }
    for scope in &mut body.scopes {
        layout.span(&mut scope.span)?;
    }
    relocate_parameters(&mut layout, facts, body)?;
    for decl in &mut body.locals {
        layout.span(&mut decl.span)?;
    }
    let sites = positions.nodes.direct_call_sites().collect::<BTreeMap<_, _>>();
    for statement in &mut body.block.stmts {
        layout.span(&mut statement.span)?;
        let ir::StatementKind::Call {
            callee: ir::Callee::Function(ir::CallableTarget::Named(call)),
            ..
        } = &mut statement.kind
        else {
            continue;
        };
        let target = call.canonical.as_mut()?;
        if target.origin == SymbolOrigin::Builtin && call.builtin.is_some() && call.direct_call_id.is_none() {
            continue;
        }
        if target.kind != SemanticSourceTargetKind::Function {
            return None;
        }
        let callee = sites.get(&statement.span)?;
        let resolved = local
            .references
            .resolved_identities
            .get(callee)
            .or_else(|| local.references.resolved_identities.get(&statement.span))?;
        if let Some(id) = &mut call.direct_call_id {
            direct_id(id, target, resolved)?;
        }
        layout.symbol(target, resolved)?;
    }
    (positions.physical_inputs_in(facts, local, records, Some(&layout))? == expected).then_some(())
}
