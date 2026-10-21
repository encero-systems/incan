//! Project use-site carrier identities, retained after checked Body IR construction and before publication.
//!
//! Source names remain diagnostics. Carrier keys encode a checked nominal identity and are accompanied by that
//! identity in each frame's existing type map. Legacy checker tables and generated source are never changed.

use constructor_arguments::ConstructorCarriers;
use std::collections::BTreeMap;

use super::{CanonicalSymbolId, IncanType, TypeCheckInfo, ast, bir, semantic_type_from_resolved};
use incan_semantics_core::body_ir::nominal_carrier_key;

type Identities = BTreeMap<String, CanonicalSymbolId>;

mod constructor_arguments;

/// Retain the exact checked pattern target before projection erases its source alias or package-qualified spelling.
/// A wildcard has no payload local from which a later pass could recover this declaration-owned identity.
pub(super) fn retain_checked_site_type(ty: &mut IncanType, span: ast::Span, facts: &TypeCheckInfo) {
    let Some(checked) = facts.expr_type(span) else {
        return;
    };
    retain_checked_type(ty, checked, facts);
}

/// Preserve a checked type's exact owner before an alias or package-qualified leaf is normalized for Body IR.
/// Project-aware builders use this for inferred call arguments and other typed operations that have no result local
/// from which the final carrier pass could recover the original binding.
pub(super) fn retain_checked_type(ty: &mut IncanType, checked: &crate::symbols::ResolvedType, facts: &TypeCheckInfo) {
    let checked = semantic_type_from_resolved(checked);
    let mut leaves = Vec::new();
    super::named_leaf_paths(&checked, &mut Vec::new(), &mut leaves);
    let identities = leaves
        .into_iter()
        .filter_map(|(_, leaf)| {
            let super::RetainedTypeLeaf::Named(name) = leaf else {
                return None;
            };
            facts
                .declarations
                .named_type_identities
                .get(&name)
                .or_else(|| facts.declarations.body_ir_native_type_identities.get(&name))
                .map(|identity| (name, identity.clone()))
        })
        .collect();
    qualify_type(ty, Some(&checked), &identities);
}

/// Project a selected declaration's own checked field binding, without consulting a consumer's spelling table.
pub(super) fn retain_declared_type(ty: &mut IncanType, identities: &Identities) {
    qualify_type(ty, None, identities);
}

/// Retain source nominal identity on every admitted project's type-bearing declaration and deferred frame.
pub(super) fn retain(module: &mut bir::BodyIrModule, facts: &TypeCheckInfo, context: &[bir::BodyIrModule]) {
    let returns = return_carriers(module, context);
    let constructors = constructor_arguments::collect(module, context);
    for declaration in &mut module.nominal_declarations {
        for ty in &mut declaration.field_types {
            qualify_type(ty, None, &declaration.named_type_identities);
        }
        if let Some(body) = &mut declaration.field_default_body {
            retain_body(body, facts, &returns, &constructors);
        }
        for partial in &mut declaration.method_partial_defaults {
            retain_body(&mut partial.frame, facts, &returns, &constructors);
        }
    }
    for alias in &mut module.type_alias_declarations {
        qualify_type(&mut alias.ty, None, &alias.named_type_identities);
    }
    for body in &mut module.bodies {
        retain_body(body, facts, &returns, &constructors);
    }
    for binding in &mut module.decorated_bindings {
        retain_body(&mut binding.original_body, facts, &returns, &constructors);
    }
    for storage in &mut module.static_declarations {
        if let bir::StaticInitializer::Computed(body) = &mut storage.initial {
            retain_body(body, facts, &returns, &constructors);
            storage.ty = body.return_type.clone();
        }
    }
}

/// Use the original checked leaf where projection erased its qualified name; otherwise keep the retained type.
fn qualify_type(ty: &mut IncanType, checked: Option<&IncanType>, identities: &Identities) {
    match ty {
        IncanType::Named(name) => {
            let selected = match checked {
                Some(IncanType::Named(original)) => identities
                    .get(original)
                    .filter(|identity| identity.declaration_name == *name || original == name),
                _ => None,
            }
            .or_else(|| identities.get(name));
            if let Some(key) = selected.and_then(nominal_carrier_key) {
                *name = key;
            }
        }
        IncanType::Generic { args, .. } | IncanType::Tuple(args) => {
            let original = match checked {
                Some(IncanType::Generic { args, .. }) | Some(IncanType::Tuple(args)) => Some(args.as_slice()),
                _ => None,
            };
            for (index, element) in args.iter_mut().enumerate() {
                qualify_type(element, original.and_then(|types| types.get(index)), identities);
            }
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) | IncanType::TypeToken(inner) => {
            let original = match checked {
                Some(IncanType::Ref(inner) | IncanType::RefMut(inner) | IncanType::TypeToken(inner)) => {
                    Some(inner.as_ref())
                }
                _ => None,
            };
            qualify_type(inner, original, identities);
        }
        IncanType::Function { params, return_type } => {
            for parameter in params {
                qualify_type(&mut parameter.ty, None, identities);
            }
            qualify_type(return_type, None, identities);
        }
        _ => {}
    }
}

/// Retain a frame's carrier independently from the caller; defaults and closures continue to use that same frame.
fn retain_body(
    body: &mut bir::Body,
    facts: &TypeCheckInfo,
    returns: &ReturnCarriers,
    constructors: &ConstructorCarriers,
) {
    retain_selected_returns(
        &body.block.stmts,
        &mut body.locals,
        &mut body.named_type_identities,
        returns,
    );
    retain_constructor_results(
        &body.block.stmts,
        &mut body.locals,
        &mut body.named_type_identities,
        constructors,
    );
    for parameter in &body.params {
        if let bir::CallableParamDefault::Source(default) = &parameter.default {
            retain_selected_returns(
                &default.stmts,
                &mut body.locals,
                &mut body.named_type_identities,
                returns,
            );
            retain_constructor_results(
                &default.stmts,
                &mut body.locals,
                &mut body.named_type_identities,
                constructors,
            );
        }
    }
    let identities = &body.named_type_identities;
    let binding = body.canonical.as_ref().and_then(|canonical| {
        facts
            .declarations
            .function_bindings_by_span
            .values()
            .chain(facts.declarations.method_bindings_by_span.values())
            .find(|binding| binding.identity.as_ref() == Some(canonical))
    });
    let checked_return = binding.map(|binding| semantic_type_from_resolved(&binding.return_type));
    qualify_type(&mut body.return_type, checked_return.as_ref(), identities);
    for local in &mut body.locals {
        let span = ast::Span::new(local.span.start, local.span.end);
        let checked = facts
            .expr_type(span)
            .or_else(|| {
                local
                    .name
                    .as_deref()
                    .and_then(|name| facts.resolved_write_type(span, name))
            })
            .map(semantic_type_from_resolved);
        qualify_type(&mut local.ty, checked.as_ref(), identities);
    }
    for parameter in &mut body.params {
        parameter.ty = body.locals[parameter.local.index()].ty.clone();
        retain_default(&mut parameter.default, identities);
    }
    retain_statements(&mut body.block.stmts, identities);
}

/// A checked construction selects its destination carrier directly, even when another import has the same name.
fn retain_constructor_results(
    statements: &[bir::Statement],
    locals: &mut [bir::LocalDecl],
    identities: &mut Identities,
    constructors: &ConstructorCarriers,
) {
    for statement in statements {
        match &statement.kind {
            bir::StatementKind::Assign { place, rvalue } => {
                if let bir::Rvalue::Aggregate(bir::AggregateKind::Constructor(target), arguments) = rvalue {
                    retain_constructed_local(place, target, locals, identities);
                    constructor_arguments::retain(target, arguments, locals, identities, constructors);
                }
                if let bir::Rvalue::Use(bir::Operand::Place(source)) = rvalue {
                    retain_copied_local(place, &source.place, locals, identities);
                }
                match rvalue {
                    bir::Rvalue::BinaryOp(bir::BinOp::Eq | bir::BinOp::Ne, left, right) => {
                        constructor_arguments::retain_comparison(left, right, locals, identities, constructors);
                    }
                    bir::Rvalue::Closure { body, .. } => {
                        retain_constructor_results(&body.stmts, locals, identities, constructors)
                    }
                    bir::Rvalue::Generator { body, .. } => {
                        retain_constructor_results(&body.stmts, locals, identities, constructors)
                    }
                    bir::Rvalue::Match { arms, .. } => {
                        for arm in arms {
                            retain_constructor_results(&arm.guard_stmts, locals, identities, constructors);
                            retain_constructor_results(&arm.body_stmts, locals, identities, constructors);
                        }
                    }
                    _ => {}
                }
            }
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                retain_constructor_results(&then_block.stmts, locals, identities, constructors);
                if let Some(block) = else_block {
                    retain_constructor_results(&block.stmts, locals, identities, constructors);
                }
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                retain_constructor_results(&body.stmts, locals, identities, constructors);
            }
            bir::StatementKind::Race { arms, .. } => {
                for arm in arms {
                    retain_constructor_results(&arm.body.stmts, locals, identities, constructors);
                }
            }
            _ => {}
        }
    }
}

/// An unprojected assignment keeps its value's already proved nominal ABI without retagging a different checked owner.
/// Only an exact canonical carrier in this frame authorizes the copy; bare source names supply no new authority.
fn retain_copied_local(
    destination: &bir::Place,
    source: &bir::Place,
    locals: &mut [bir::LocalDecl],
    identities: &Identities,
) {
    if !destination.projection.is_empty() || !source.projection.is_empty() {
        return;
    }
    let Some(IncanType::Named(key)) = source
        .local_id()
        .and_then(|id| locals.get(id.index()))
        .map(|local| &local.ty)
    else {
        return;
    };
    let Some(owner) = identities
        .get(key)
        .filter(|owner| nominal_carrier_key(owner).as_ref() == Some(key))
    else {
        return;
    };
    let retained = IncanType::Named(key.clone());
    let Some(local) = destination.local_id().and_then(|id| locals.get_mut(id.index())) else {
        return;
    };
    if matches!(&local.ty, IncanType::Named(name)
        if (name == &owner.declaration_name || identities.get(name) == Some(owner))
            && identities.get(name).is_none_or(|checked| checked == owner))
    {
        local.ty = retained;
    }
}

/// Require the checker-selected source constructor and destination type to agree before retaining its identity.
fn retain_constructed_local(
    place: &bir::Place,
    target: &bir::ConstructorTarget,
    locals: &mut [bir::LocalDecl],
    identities: &mut Identities,
) {
    if !place.projection.is_empty() || target.direct_declaration_id.is_none() {
        return;
    }
    let Some(identity) = &target.canonical else { return };
    let Some(key) = nominal_carrier_key(identity) else {
        return;
    };
    let Some(local) = place.local_id().and_then(|id| locals.get_mut(id.index())) else {
        return;
    };
    let IncanType::Named(name) = &mut local.ty else { return };
    if *name != identity.declaration_name && *name != target.name && identities.get(name) != Some(identity) {
        return;
    }
    *name = key.clone();
    identities.insert(key, identity.clone());
}

/// Deferred computations carry the declaring frame's canonical type map and never read a caller's namespace.
fn retain_default(default: &mut bir::CallableParamDefault, identities: &Identities) {
    if let bir::CallableParamDefault::Source(computation) = default {
        retain_statements(&mut computation.stmts, identities);
        retain_operand(&mut computation.result, identities);
    }
}

/// Qualify nested type-bearing operations without changing execution order, ownership or target selection.
fn retain_statements(statements: &mut [bir::Statement], identities: &Identities) {
    for statement in statements {
        retain_statement_operands(&mut statement.kind, identities);
        match &mut statement.kind {
            bir::StatementKind::Assign { rvalue, .. } => retain_rvalue(rvalue, identities),
            bir::StatementKind::Call { callee, .. } => retain_callee(callee, identities),
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                retain_statements(&mut then_block.stmts, identities);
                if let Some(block) = else_block {
                    retain_statements(&mut block.stmts, identities);
                }
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                retain_statements(&mut body.stmts, identities);
            }
            bir::StatementKind::Race { arms, .. } => {
                for arm in arms {
                    retain_statements(&mut arm.body.stmts, identities);
                }
            }
            bir::StatementKind::TryPropagate { error_routing, .. } => match error_routing {
                bir::TryErrorRouting::SameType { error_type } => qualify_type(error_type, None, identities),
                bir::TryErrorRouting::ConversionRequired {
                    source_error_type,
                    destination_error_type,
                } => {
                    qualify_type(source_error_type, None, identities);
                    qualify_type(destination_error_type, None, identities);
                }
                bir::TryErrorRouting::Unresolved => {}
            },
            _ => {}
        }
    }
}

/// Carry canonical nominal leaves through explicit function and method instantiations already selected by checking.
fn retain_callee(callee: &mut bir::Callee, identities: &Identities) {
    let arguments = match callee {
        bir::Callee::Function(bir::CallableTarget::Named(target)) => {
            if let Some(receiver) = &mut target.receiver_type {
                qualify_type(receiver, None, identities);
            }
            &mut target.type_args
        }
        bir::Callee::Method(target) => {
            if let Some(selection) = &mut target.native_trait_delegation {
                for ty in selection
                    .parameters
                    .iter_mut()
                    .chain(std::iter::once(&mut selection.return_type))
                    .chain(&mut selection.native_type_arguments)
                {
                    qualify_type(ty, None, identities);
                }
            }
            &mut target.type_args
        }
        _ => return,
    };
    for argument in arguments {
        qualify_type(argument, None, identities);
    }
}

/// Keep represented closure/default/match computations on the same checked carrier as their enclosing frame.
fn retain_rvalue(value: &mut bir::Rvalue, identities: &Identities) {
    retain_rvalue_operands(value, identities);
    match value {
        bir::Rvalue::ResultVariant(variant) => {
            qualify_type(&mut variant.ok_type, None, identities);
            qualify_type(&mut variant.error_type, None, identities);
        }
        bir::Rvalue::IsInstance { value_ty, target, .. } => {
            qualify_type(value_ty, None, identities);
            qualify_type(&mut target.ty, None, identities);
        }
        bir::Rvalue::Closure { params, body, .. } => {
            for parameter in params {
                qualify_type(&mut parameter.ty, None, identities);
                retain_default(&mut parameter.default, identities);
            }
            retain_statements(&mut body.stmts, identities);
        }
        bir::Rvalue::Generator { body, .. } => retain_statements(&mut body.stmts, identities),
        bir::Rvalue::Match { arms, .. } => {
            for arm in arms {
                retain_pattern(&mut arm.pattern, identities);
                retain_statements(&mut arm.guard_stmts, identities);
                retain_statements(&mut arm.body_stmts, identities);
            }
        }
        _ => {}
    }
}

/// Keep nominal global storage and union/Option payload projections on their already checked carrier.
fn retain_place(place: &mut bir::Place, identities: &Identities) {
    if let bir::PlaceRoot::Global(global) = &mut place.root {
        qualify_type(&mut global.ty, None, identities);
    }
    for projection in &mut place.projection {
        match projection {
            bir::PlaceElem::UnionMember { ty } => qualify_type(ty, None, identities),
            bir::PlaceElem::OptionPayload {
                option_type,
                payload_type,
            } => {
                qualify_type(option_type, None, identities);
                qualify_type(payload_type, None, identities);
            }
            bir::PlaceElem::Index(index) => retain_operand(index, identities),
            bir::PlaceElem::Slice { start, end, step } => {
                for operand in start.iter_mut().chain(end).chain(step) {
                    retain_operand(operand, identities);
                }
            }
            bir::PlaceElem::Field { .. } => {}
        }
    }
}

/// Preserve a read's ownership and source position while qualifying only its type-bearing storage projections.
fn retain_operand(operand: &mut bir::Operand, identities: &Identities) {
    match operand {
        bir::Operand::Place(read) => retain_place(&mut read.place, identities),
        bir::Operand::Constant(bir::Constant::FrozenAggregate(ty, _)) => qualify_type(ty, None, identities),
        bir::Operand::Constant(_) => {}
    }
}

/// Carry the same qualified storage through fixed, named and spread argument operands.
fn retain_elements(elements: &mut [bir::ArgumentElement], identities: &Identities) {
    for element in elements {
        match element {
            bir::ArgumentElement::One(operand) | bir::ArgumentElement::Named { operand, .. } => {
                retain_operand(operand, identities)
            }
            bir::ArgumentElement::Spread(spread) => retain_operand(&mut spread.source, identities),
        }
    }
}

/// Qualify immediate storage reads and writes separately from nested statement computations.
fn retain_statement_operands(statement: &mut bir::StatementKind, identities: &Identities) {
    match statement {
        bir::StatementKind::Assign { place, .. } => retain_place(place, identities),
        bir::StatementKind::Call {
            destination,
            callee,
            args,
            ..
        } => {
            if let Some(place) = destination {
                retain_place(place, identities);
            }
            retain_elements(args, identities);
            if let bir::Callee::Function(bir::CallableTarget::Local(target)) = callee {
                retain_place(&mut target.operand.place, identities);
            }
        }
        bir::StatementKind::If { cond, .. } => retain_operand(cond, identities),
        bir::StatementKind::Break { value } | bir::StatementKind::Return { value } => {
            if let Some(value) = value {
                retain_operand(value, identities);
            }
        }
        bir::StatementKind::Yield { value } | bir::StatementKind::Expr { value } => retain_operand(value, identities),
        bir::StatementKind::Await { destination, awaited } => {
            if let Some(place) = destination {
                retain_place(place, identities);
            }
            retain_operand(awaited, identities);
        }
        bir::StatementKind::TryPropagate {
            destination, operand, ..
        } => {
            retain_place(destination, identities);
            retain_operand(operand, identities);
        }
        bir::StatementKind::IterNext {
            destination, iterator, ..
        } => {
            retain_place(destination, identities);
            retain_operand(iterator, identities);
        }
        bir::StatementKind::Race { destination, arms } => {
            if let Some(place) = destination {
                retain_place(place, identities);
            }
            for arm in arms {
                retain_operand(&mut arm.awaitable, identities);
                retain_operand(&mut arm.result, identities);
            }
        }
        bir::StatementKind::Assert { kind, message, .. } => {
            if let Some(message) = message {
                retain_operand(message, identities);
            }
            match kind {
                bir::AssertionKind::Condition { cond } => retain_operand(cond, identities),
                bir::AssertionKind::Raises { call, .. } => retain_operand(call, identities),
                bir::AssertionKind::Pattern { scrutinee, pattern } => {
                    retain_operand(scrutinee, identities);
                    retain_pattern(pattern, identities);
                }
            }
        }
        _ => {}
    }
}

/// Qualify rvalue storage reads without changing the order in which an expression evaluates them.
fn retain_rvalue_operands(value: &mut bir::Rvalue, identities: &Identities) {
    match value {
        bir::Rvalue::Use(operand) | bir::Rvalue::UnaryOp(_, operand) => retain_operand(operand, identities),
        bir::Rvalue::BinaryOp(_, left, right) => {
            retain_operand(left, identities);
            retain_operand(right, identities);
        }
        bir::Rvalue::IsInstance { value, .. } => retain_operand(value, identities),
        bir::Rvalue::Aggregate(_, elements) => retain_elements(elements, identities),
        bir::Rvalue::Dict(entries) => {
            for entry in entries {
                match entry {
                    bir::DictEntry::Pair(key, value) => {
                        retain_operand(key, identities);
                        retain_operand(value, identities);
                    }
                    bir::DictEntry::Spread(spread) => retain_operand(&mut spread.source, identities),
                }
            }
        }
        bir::Rvalue::ResultVariant(variant) => retain_operand(&mut variant.payload, identities),
        bir::Rvalue::Format(parts) => {
            for part in parts {
                if let bir::FormatPart::Expr { operand, .. } = part {
                    retain_operand(operand, identities);
                }
            }
        }
        bir::Rvalue::Closure {
            captured_operands,
            body,
            ..
        } => {
            for operand in captured_operands {
                retain_operand(operand, identities);
            }
            retain_operand(&mut body.result, identities);
        }
        bir::Rvalue::Generator {
            source,
            captured_operands,
            ..
        } => {
            retain_operand(source, identities);
            for operand in captured_operands {
                retain_operand(operand, identities);
            }
        }
        bir::Rvalue::Match { scrutinee, arms } => {
            retain_operand(scrutinee, identities);
            for arm in arms {
                if let Some(guard) = &mut arm.guard {
                    retain_operand(guard, identities);
                }
                retain_operand(&mut arm.result, identities);
            }
        }
        _ => {}
    }
}

/// A union pattern's checked member carrier follows its original module; field bindings keep their existing locals.
fn retain_pattern(pattern: &mut bir::Pattern, identities: &Identities) {
    match pattern {
        bir::Pattern::UnionMember { ty, fields } => {
            qualify_type(ty, None, identities);
            for field in fields {
                retain_pattern(field, identities);
            }
        }
        bir::Pattern::Tuple(fields)
        | bir::Pattern::Or(fields)
        | bir::Pattern::Result { fields, .. }
        | bir::Pattern::Enum { fields, .. } => {
            for field in fields {
                retain_pattern(field, identities);
            }
        }
        bir::Pattern::Nominal { fields, .. } => {
            for (_, field) in fields {
                retain_pattern(field, identities);
            }
        }
        _ => {}
    }
}
/// One declaring frame's checked result and the canonical bindings that give its nominal leaves meaning.
struct ReturnCarrier {
    ty: IncanType,
    identities: Identities,
}

type ReturnCarriers = BTreeMap<CanonicalSymbolId, ReturnCarrier>;

/// Snapshot only declaration-owned return facts before mutating this module's execution frames.
fn return_carriers(module: &bir::BodyIrModule, context: &[bir::BodyIrModule]) -> ReturnCarriers {
    context
        .iter()
        .flat_map(|module| &module.bodies)
        .chain(&module.bodies)
        .filter_map(|body| {
            body.canonical.as_ref().map(|identity| {
                (
                    identity.clone(),
                    ReturnCarrier {
                        ty: body.return_type.clone(),
                        identities: body.named_type_identities.clone(),
                    },
                )
            })
        })
        .collect()
}

/// A call's selected canonical declaration supplies a nominal result's declaring context, not the caller's namespace.
fn retain_selected_returns(
    statements: &[bir::Statement],
    locals: &mut [bir::LocalDecl],
    identities: &mut Identities,
    returns: &ReturnCarriers,
) {
    for statement in statements {
        match &statement.kind {
            bir::StatementKind::Call {
                destination: Some(place),
                callee,
                ..
            } if place.projection.is_empty() => {
                let selected = match callee {
                    bir::Callee::Function(bir::CallableTarget::Named(target)) => target.canonical.as_ref(),
                    bir::Callee::Method(target) => target.canonical.as_ref(),
                    _ => None,
                };
                let Some(carrier) = selected.and_then(|identity| returns.get(identity)) else {
                    continue;
                };
                let Some(local) = place.local_id().and_then(|id| locals.get_mut(id.index())) else {
                    continue;
                };
                retain_declared_return_type(&mut local.ty, &carrier.ty, &carrier.identities, identities);
            }
            bir::StatementKind::Assign { rvalue, .. } => match rvalue {
                bir::Rvalue::Closure { body, .. } => retain_selected_returns(&body.stmts, locals, identities, returns),
                bir::Rvalue::Generator { body, .. } => {
                    retain_selected_returns(&body.stmts, locals, identities, returns)
                }
                bir::Rvalue::Match { arms, .. } => {
                    for arm in arms {
                        retain_selected_returns(&arm.guard_stmts, locals, identities, returns);
                        retain_selected_returns(&arm.body_stmts, locals, identities, returns);
                    }
                }
                _ => {}
            },
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                retain_selected_returns(&then_block.stmts, locals, identities, returns);
                if let Some(block) = else_block {
                    retain_selected_returns(&block.stmts, locals, identities, returns);
                }
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                retain_selected_returns(&body.stmts, locals, identities, returns)
            }
            bir::StatementKind::Race { arms, .. } => {
                for arm in arms {
                    retain_selected_returns(&arm.body.stmts, locals, identities, returns);
                }
            }
            _ => {}
        }
    }
}

/// Qualify only concrete leaves that agree with the selected declaration's result; type parameters grant no carrier.
///
/// A leaf agrees when it still carries the declared spelling, or when a construction-time pass already qualified that
/// same spelling through the caller's own name table. Checked types name nominals by spelling alone, so a caller that
/// declares its own `EvidenceId` would otherwise keep its local owner on an empty list or `None` passed to a foreign
/// `list[EvidenceId]` field. The selected declaration is the checked authority for the whole temporary it fills.
fn retain_declared_return_type(
    ty: &mut IncanType,
    declared: &IncanType,
    source: &Identities,
    destination: &mut Identities,
) {
    match (ty, declared) {
        (IncanType::Named(name), IncanType::Named(declared)) => {
            let Some(identity) = source.get(declared) else { return };
            let Some(key) = nominal_carrier_key(identity) else {
                return;
            };
            let same_spelling = destination.get(name).is_some_and(|current| {
                current.declaration_name == identity.declaration_name && current.kind == identity.kind
            });
            if *name == *declared || *name == identity.declaration_name || same_spelling {
                *name = key.clone();
                destination.insert(key, identity.clone());
            }
        }
        (
            IncanType::Generic { base, args },
            IncanType::Generic {
                base: declared_base,
                args: declared_args,
            },
        ) if base == declared_base && args.len() == declared_args.len() => {
            for (argument, declared) in args.iter_mut().zip(declared_args) {
                retain_declared_return_type(argument, declared, source, destination);
            }
        }
        (IncanType::Tuple(args), IncanType::Tuple(declared_args)) if args.len() == declared_args.len() => {
            for (argument, declared) in args.iter_mut().zip(declared_args) {
                retain_declared_return_type(argument, declared, source, destination);
            }
        }
        _ => {}
    }
}
