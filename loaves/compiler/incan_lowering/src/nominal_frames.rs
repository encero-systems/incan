//! Checked callable frames that contribute nominal layouts without adding executable bodies.

use std::collections::{BTreeSet, VecDeque};

use incan_semantics_core::body_ir::{
    ArgumentElement, Block, Body, BodyIrModule, CallableTarget, Callee, Constant, MatchArm, MethodTarget, Operand,
    Rvalue, StatementKind, StaticInitializer,
};
use incan_semantics_core::{CanonicalSymbolId, IncanType, SemanticSourceTargetKind};

use crate::native_type_identity::ConstructorIndex;
use crate::type_parameters::{UNBOUND_TYPE_PARAMETER, indexed_static_type_arguments, instantiate, substitute};

/// One selected method identity together with the checked receiver that owns this invocation.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectedMember {
    pub identity: CanonicalSymbolId,
    pub receiver: IncanType,
}

/// Inventory selected member identities in one executable frame without admitting unused protocol defaults.
pub fn called_members(body: &Body) -> Vec<SelectedMember> {
    let mut selected = Vec::new();
    collect_member_block(body, &body.block, &mut selected);
    selected
}

fn collect_member_block(body: &Body, block: &Block, selected: &mut Vec<SelectedMember>) {
    for statement in &block.stmts {
        collect_member_statement(body, &statement.kind, selected);
    }
}

fn collect_member_statement(body: &Body, kind: &StatementKind, selected: &mut Vec<SelectedMember>) {
    match kind {
        StatementKind::Call {
            callee: Callee::Method(target),
            args,
            ..
        } => {
            if let Some(first) = args.first() {
                collect_member_target(body, target, first, selected);
            }
        }
        StatementKind::If {
            then_block, else_block, ..
        } => {
            collect_member_block(body, then_block, selected);
            if let Some(nested) = else_block {
                collect_member_block(body, nested, selected);
            }
        }
        StatementKind::Loop { body: nested } => collect_member_block(body, nested, selected),
        StatementKind::UnsafeRegion { body: nested, .. } => collect_member_block(body, nested, selected),
        StatementKind::Race { arms, .. } => {
            for arm in arms {
                collect_member_block(body, &arm.body, selected);
            }
        }
        StatementKind::Assign { rvalue, .. } => collect_member_value(body, rvalue, selected),
        _ => {}
    }
}

fn collect_member_value(body: &Body, value: &Rvalue, selected: &mut Vec<SelectedMember>) {
    if let Rvalue::Match { arms, .. } = value {
        for arm in arms {
            collect_member_arm(body, arm, selected);
        }
    }
}

fn collect_member_target(
    body: &Body,
    target: &MethodTarget,
    argument: &ArgumentElement,
    selected: &mut Vec<SelectedMember>,
) {
    let Some(receiver) = selected_receiver_type(body, argument) else {
        return;
    };
    if let Some(identity) = &target.canonical {
        if selected
            .iter()
            .any(|retained| retained.identity == *identity && retained.receiver == receiver)
        {
            return;
        }
        selected.push(SelectedMember {
            identity: identity.clone(),
            receiver,
        });
    }
}

fn collect_member_arm(body: &Body, arm: &MatchArm, selected: &mut Vec<SelectedMember>) {
    for statement in &arm.guard_stmts {
        collect_member_statement(body, &statement.kind, selected);
    }
    for statement in &arm.body_stmts {
        collect_member_statement(body, &statement.kind, selected);
    }
}

/// A checked local receiver or static type token; projected receivers grant no unproven owner.
fn selected_receiver_type(body: &Body, argument: &ArgumentElement) -> Option<IncanType> {
    match argument {
        ArgumentElement::One(value) => selected_operand_type(body, value),
        _ => None,
    }
}

fn selected_operand_type(body: &Body, value: &Operand) -> Option<IncanType> {
    match value {
        Operand::Constant(Constant::TypeToken(ty)) => Some(ty.clone()),
        Operand::Constant(_) => None,
        Operand::Place(read) => {
            if !read.place.projection.is_empty() {
                return None;
            }
            read.place.local_id().map(|local| body.locals[local.index()].ty.clone())
        }
    }
}

/// One canonical source callable and the checked arguments closing its frame, with its call ancestry.
struct ScheduledFrame {
    body: Body,
    arguments: Vec<IncanType>,
    ancestors: Vec<String>,
}

/// Drain layout requests in discovery order while retaining only exact checked keys after processing.
#[derive(Default)]
struct ScheduledFrames {
    pending: VecDeque<ScheduledFrame>,
    seen: BTreeSet<(CanonicalSymbolId, Vec<IncanType>)>,
}

/// Checked return, parameter and local types from retained declarations, without copying their bodies.
pub fn declaration_storage(module: &BodyIrModule) -> Vec<IncanType> {
    let mut types = Vec::new();
    for callable in &module.loaf_callables {
        if let Some(body) = callable.declaration_frame() {
            types.push(body.return_type.clone());
            types.extend(body.params.iter().map(|parameter| parameter.ty.clone()));
            types.extend(body.locals.iter().map(|local| local.ty.clone()));
        }
    }
    types
}

/// Collect source, static and closed callable frames for layout discovery without granting execution.
pub fn nominal_frames(constructors: &ConstructorIndex, module: &BodyIrModule) -> Result<Vec<Body>, String> {
    let mut frames = Vec::new();
    let mut pending = ScheduledFrames::default();
    for body in &module.bodies {
        frames.push(body.clone());
        if body.type_parameters.is_empty() {
            schedule_block(constructors, module, &body.block, &[], &[], &mut pending, &[])?;
        }
    }
    for declaration in &module.static_declarations {
        if let StaticInitializer::Computed(body) = &declaration.initial {
            frames.push((**body).clone());
            schedule_block(constructors, module, &body.block, &[], &[], &mut pending, &[])?;
        }
    }
    while let Some(request) = pending.pending.pop_front() {
        // This inventory closes layout types only; executable callable ownership is selected by call lowering. A
        // method or property whose owner placeholders the recorded arguments leave open grants no layout; every other
        // instantiation failure is refused.
        match instantiate(&request.body, &request.arguments, &[]) {
            Ok(frame) => frames.push(frame),
            Err(error) => {
                if owner_dependent(&request.body) && error.starts_with(UNBOUND_TYPE_PARAMETER) {
                    continue;
                }
                return Err(error);
            }
        }
        let Some(identity) = &request.body.canonical else {
            return Err("unsupported Body IR nominal frame without canonical identity".into());
        };
        let mut ancestry = request.ancestors.clone();
        ancestry.push(identity.render_compact());
        schedule_block(
            constructors,
            module,
            &request.body.block,
            &request.body.type_parameters,
            &request.arguments,
            &mut pending,
            &ancestry,
        )?;
    }
    Ok(frames)
}

/// Methods and properties close their owner's placeholders through the receiver; plain functions never do.
fn owner_dependent(body: &Body) -> bool {
    body.canonical
        .as_ref()
        .is_some_and(|identity| identity.kind != SemanticSourceTargetKind::Function)
}

fn schedule_block(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    block: &Block,
    names: &[String],
    arguments: &[IncanType],
    pending: &mut ScheduledFrames,
    ancestors: &[String],
) -> Result<(), String> {
    for statement in &block.stmts {
        schedule_statement(
            constructors,
            module,
            &statement.kind,
            names,
            arguments,
            pending,
            ancestors,
        )?;
    }
    Ok(())
}

fn schedule_statement(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    kind: &StatementKind,
    names: &[String],
    arguments: &[IncanType],
    pending: &mut ScheduledFrames,
    ancestors: &[String],
) -> Result<(), String> {
    match kind {
        StatementKind::Call { callee, .. } => match callee {
            Callee::Function(CallableTarget::Named(target)) => {
                let call_types = indexed_static_type_arguments(constructors, target)?;
                schedule_call(
                    module,
                    &target.canonical,
                    &call_types,
                    names,
                    arguments,
                    pending,
                    ancestors,
                )?;
            }
            Callee::Method(target) => {
                schedule_call(
                    module,
                    &target.canonical,
                    &target.type_args,
                    names,
                    arguments,
                    pending,
                    ancestors,
                )?;
            }
            _ => {}
        },
        StatementKind::Assign {
            rvalue: Rvalue::Match { arms, .. },
            ..
        } => {
            for arm in arms {
                for statement in arm.guard_stmts.iter().chain(&arm.body_stmts) {
                    schedule_statement(
                        constructors,
                        module,
                        &statement.kind,
                        names,
                        arguments,
                        pending,
                        ancestors,
                    )?;
                }
            }
        }
        StatementKind::Race { arms, .. } => {
            for arm in arms {
                schedule_block(constructors, module, &arm.body, names, arguments, pending, ancestors)?;
            }
        }
        StatementKind::Loop { body } => {
            schedule_block(constructors, module, body, names, arguments, pending, ancestors)?
        }
        StatementKind::UnsafeRegion { body, .. } => {
            schedule_block(constructors, module, body, names, arguments, pending, ancestors)?
        }
        StatementKind::If {
            then_block, else_block, ..
        } => {
            schedule_block(constructors, module, then_block, names, arguments, pending, ancestors)?;
            if let Some(nested) = else_block {
                schedule_block(constructors, module, nested, names, arguments, pending, ancestors)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Queue a unique closed source call; unresolved binders and recursive ancestry contribute no layout.
fn schedule_call(
    module: &BodyIrModule,
    canonical: &Option<CanonicalSymbolId>,
    type_args: &[IncanType],
    names: &[String],
    arguments: &[IncanType],
    pending: &mut ScheduledFrames,
    ancestors: &[String],
) -> Result<(), String> {
    if type_args.is_empty() {
        return Ok(());
    }
    let Some(identity) = canonical else {
        return Ok(());
    };
    if identity.kind != SemanticSourceTargetKind::Function
        && identity.kind != SemanticSourceTargetKind::Method
        && identity.kind != SemanticSourceTargetKind::Property
    {
        return Ok(());
    }
    if ancestors.contains(&identity.render_compact()) {
        return Ok(());
    }
    let Some(body) = checked_callable_body(module, identity)? else {
        return Ok(());
    };
    if body.type_parameters.is_empty() || body.type_parameters.len() != type_args.len() {
        return Ok(());
    }
    let mut closed = Vec::with_capacity(type_args.len());
    for argument in type_args {
        match substitute(argument, names, arguments) {
            Ok(value) => closed.push(value),
            Err(_) => return Ok(()),
        }
    }
    if !closed_arguments(&closed) {
        return Ok(());
    }
    let key = (identity.clone(), closed.clone());
    if pending.seen.contains(&key) {
        return Ok(());
    }
    pending.seen.insert(key);
    pending.pending.push_back(ScheduledFrame {
        body,
        arguments: closed,
        ancestors: ancestors.to_vec(),
    });
    Ok(())
}

/// Require closed argument trees; native carrier admission validates the remaining type forms.
pub fn closed_arguments(arguments: &[IncanType]) -> bool {
    arguments.iter().all(closed_type)
}

/// Require every structural component to be closed, including borrowed referents and callable inputs and output.
pub fn closed_type(ty: &IncanType) -> bool {
    match ty {
        IncanType::TypeVar(_) | IncanType::SelfType | IncanType::Infer | IncanType::Unknown => false,
        IncanType::Generic { args, .. } => args.iter().all(closed_type),
        IncanType::Tuple(elements) => elements.iter().all(closed_type),
        IncanType::Ref(inner) | IncanType::RefMut(inner) | IncanType::TypeToken(inner) => closed_type(inner),
        IncanType::NativeArray { element, .. } => closed_type(element),
        IncanType::Function { params, return_type } => {
            params.iter().all(|parameter| closed_type(&parameter.ty)) && closed_type(return_type)
        }
        _ => true,
    }
}

/// Resolve a unique checked body; ambiguous owner-dependent method frames grant no nominal layout.
fn checked_callable_body(module: &BodyIrModule, identity: &CanonicalSymbolId) -> Result<Option<Body>, String> {
    let mut selected: Option<Body> = None;
    for body in &module.bodies {
        if body.canonical.as_ref() != Some(identity) {
            continue;
        }
        if selected.is_some() {
            if identity.kind == SemanticSourceTargetKind::Function {
                return Err("unsupported Body IR ambiguous canonical nominal frame".into());
            }
            return Ok(None);
        }
        selected = Some(body.clone());
    }
    Ok(selected)
}
