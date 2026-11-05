//! Infer shared access for plain parameters from their complete checked bodies, then publish that decision in the
//! callable signature before the executable ABI is consumed. Owned snapshots, mutable uses and escaping values keep
//! their original storage contracts.

use std::collections::HashSet;

use super::{CanonicalSymbolId, IncanType, SemanticSourceTargetKind, bir};
use incan_semantics_core::SymbolOrigin;

/// One canonical callable's already checked closed storage signature.
pub(super) struct Signature {
    identity: CanonicalSymbolId,
    generic: bool,
    arguments: Vec<IncanType>,
    parameters: Vec<SignatureParameter>,
}

/// One callable input's value type and its single checked access decision.
#[derive(Clone, PartialEq)]
struct SignatureParameter {
    ty: IncanType,
    access: incan_semantics_core::IncanInputAccess,
}

impl SignatureParameter {
    /// Return the underlying owner when this slot already receives shared access for fixed-point analysis.
    fn shared_owner(&self) -> Option<&IncanType> {
        match &self.ty {
            IncanType::Ref(owner) => Some(owner),
            owner if self.access.is_shared() => Some(owner),
            _ => None,
        }
    }

    /// Return the owner only when this analysis introduced the call loan; explicit calls were settled earlier.
    fn inferred_shared_owner(&self) -> Option<&IncanType> {
        (self.access == incan_semantics_core::IncanInputAccess::InferredShared).then_some(&self.ty)
    }
}

/// A read-only proof is local to one frame and never grants an escaping or exclusive loan.
struct ReadContext<'a> {
    locals: &'a [bir::LocalDecl],
    signatures: &'a [Signature],
    local: bir::LocalId,
    owner: &'a IncanType,
    cursor: bool,
}

/// Retain native cursors, infer plain shared parameters to a greatest fixed point and rewrite every caller from the
/// resulting signatures. A function-item closure remains the declared by-value callable adapter: only its internal
/// call lends the wrapper's owned argument to the inferred-shared target.
pub(super) fn retain(module: &mut bir::BodyIrModule, context: &[bir::BodyIrModule]) {
    let mut signatures = collect_signatures(module, context, &HashSet::new());
    for body in &mut module.bodies {
        retain_cursors(body, &signatures);
    }

    let mut candidates = candidate_parameters(module);
    loop {
        signatures = collect_signatures(module, context, &candidates);
        let before = candidates.len();
        candidates.retain(|&(body_index, parameter_index)| {
            let Some(body) = module.bodies.get(body_index) else {
                return false;
            };
            let Some(parameter) = body.params.get(parameter_index) else {
                return false;
            };
            parameter_is_read_only(body, parameter, &signatures)
        });
        if candidates.len() == before {
            break;
        }
    }

    for (body_index, parameter_index) in &candidates {
        if let Some(body) = module.bodies.get_mut(*body_index) {
            retain_shared_parameter(body, *parameter_index);
        }
    }
    signatures = collect_signatures(module, context, &HashSet::new());
    for body in &mut module.bodies {
        retain_body_calls(body, &signatures);
    }
}

/// Publish one inferred decision in both the executable slot and any already admitted checked-input profile. The
/// profile remains the authority for explicit inputs and returned views, so its complete slot inventory must not
/// drift merely because this later whole-body analysis refined an ordinary input.
fn retain_shared_parameter(body: &mut bir::Body, parameter_index: usize) {
    let Some(parameter) = body.params.get_mut(parameter_index) else {
        return;
    };
    parameter.input_access = incan_semantics_core::IncanInputAccess::InferredShared;
    if let Some(retained) = body
        .checked_input_profile
        .as_mut()
        .and_then(|profile| profile.parameters.get_mut(parameter_index))
        && retained.slot == parameter_index
    {
        retained.access = incan_semantics_core::IncanInputAccess::InferredShared;
    }
}

/// Seed every ordinary non-Copy input whose source contract leaves access inferred. The decreasing fixed point removes
/// any seed whose body moves, stores, returns, mutates or passes the value to a non-shared input. Open trait slots and
/// their checked implementations keep the trait's authored ABI because an implementation body cannot refine that
/// independently shared contract.
fn candidate_parameters(module: &bir::BodyIrModule) -> HashSet<(usize, usize)> {
    let trait_implementations = module
        .trait_implementations
        .iter()
        .map(|implementation| implementation.implementation.clone())
        .collect::<HashSet<_>>();
    module
        .bodies
        .iter()
        .enumerate()
        .filter(|(_, body)| {
            body.type_parameters.is_empty()
                && !body.is_async
                && !body.is_generator()
                && body.extern_delegation.is_none()
                && body.native_attributes.is_empty()
                && body
                    .canonical
                    .as_ref()
                    .is_none_or(|identity| !trait_implementations.contains(identity))
                && !has_open_trait_receiver(body)
        })
        .flat_map(|(body_index, body)| {
            body.params
                .iter()
                .enumerate()
                .filter(|(_, parameter)| parameter_is_candidate(body, parameter))
                .map(move |(parameter_index, _)| (body_index, parameter_index))
        })
        .collect()
}

/// Whether this is a trait-owned default body whose receiver is still the open `Self` contract. Abstract trait
/// methods have no body; concrete adopters are excluded separately through the checked implementation registry.
fn has_open_trait_receiver(body: &bir::Body) -> bool {
    body.params.first().is_some_and(|parameter| {
        parameter.ty == IncanType::SelfType
            && body
                .locals
                .get(parameter.local.index())
                .is_some_and(|local| matches!(local.origin, bir::LocalOrigin::Receiver { .. }))
    })
}

/// Keep receivers, rest packs, defaults, explicit reference carriers, callable storage, explicit modes, mutable
/// inputs and Copy values on their established passing rules. Function-item adapters may borrow inside their
/// forwarding body, but callable-valued parameters keep the by-value closure carrier that lowering supports.
/// Defaults need a retained invocation owner before they can publish a borrowed parameter contract.
fn parameter_is_candidate(body: &bir::Body, parameter: &bir::CallableParam) -> bool {
    parameter.input_access == incan_semantics_core::IncanInputAccess::Inferred
        && parameter.kind == incan_semantics_core::IncanCallableParamKind::Normal
        && body
            .params
            .iter()
            .all(|input| input.kind == incan_semantics_core::IncanCallableParamKind::Normal)
        && matches!(parameter.default, bir::CallableParamDefault::Required)
        && !parameter.mutable
        && !parameter.native_owned
        && !parameter.ty.abi_v0_facts().ownership.is_trivially_copy()
        && !matches!(parameter.ty, IncanType::Ref(_) | IncanType::RefMut(_))
        && !matches!(parameter.ty, IncanType::Function { .. })
        && !matches!(parameter.ty, IncanType::TypeVar(_) | IncanType::SelfType)
        && native_owner(&parameter.ty, &body.named_type_identities)
        && !body
            .locals
            .get(parameter.local.index())
            .is_some_and(|local| matches!(local.origin, bir::LocalOrigin::Receiver { .. }))
}

/// Prove one candidate against the current call-graph hypothesis without changing any signature.
fn parameter_is_read_only(body: &bir::Body, parameter: &bir::CallableParam, signatures: &[Signature]) -> bool {
    let context = ReadContext {
        locals: &body.locals,
        signatures,
        local: parameter.local,
        owner: &parameter.ty,
        cursor: false,
    };
    readonly(&body.block.stmts, &context)
}

/// Finalize only this body's call-loan operands from selected final signatures, including its lexical closures and
/// generators. Callee signature production and shared-loan group settlement have already finished.
pub(super) fn retain_body_calls(body: &mut bir::Body, signatures: &[Signature]) {
    walk_calls_mut(&mut body.block.stmts, &mut |statement| {
        retain_function_item_adapter(statement, signatures);
        retain_call_loans(statement, signatures);
    });
}

/// Keep a named function's declared by-value callable shape by lowering its forwarding closure as an adapter when
/// the selected body acquired inferred shared access. Direct function-item lowering is valid only when both input
/// contracts agree; the adapter owns its callable argument and lends it only for the internal call.
fn retain_function_item_adapter(statement: &mut bir::Statement, signatures: &[Signature]) {
    let bir::StatementKind::Assign {
        rvalue: bir::Rvalue::Closure { params, body, .. },
        ..
    } = &mut statement.kind
    else {
        return;
    };
    if !body.function_item {
        return;
    }
    let [
        bir::Statement {
            kind: bir::StatementKind::Call { callee, args, .. },
            ..
        },
    ] = body.stmts.as_slice()
    else {
        return;
    };
    let adapts_inferred_shared = args.iter().enumerate().any(|(index, _)| {
        call_parameter(callee, index, signatures).is_some_and(|parameter| {
            parameter.access == incan_semantics_core::IncanInputAccess::InferredShared
                && params
                    .get(index)
                    .is_some_and(|adapter| adapter.input_access != parameter.access)
        })
    });
    if adapts_inferred_shared {
        body.function_item = false;
    }
}

/// Combine linked, previously settled module and current-body signatures without deriving a callable from its name.
fn collect_signatures(
    module: &bir::BodyIrModule,
    context: &[bir::BodyIrModule],
    candidates: &HashSet<(usize, usize)>,
) -> Vec<Signature> {
    context
        .iter()
        .flat_map(module_signatures)
        .chain(module.loaf_callables.iter().map(loaf_signature))
        .chain(
            module
                .bodies
                .iter()
                .enumerate()
                .filter_map(|(body_index, body)| body_signature(body, body_index, candidates)),
        )
        .collect()
}

/// Read already settled signatures from a dependency module.
fn module_signatures(module: &bir::BodyIrModule) -> impl Iterator<Item = Signature> + '_ {
    module.loaf_callables.iter().map(loaf_signature).chain(
        module
            .bodies
            .iter()
            .enumerate()
            .filter_map(|(body_index, body)| body_signature(body, body_index, &HashSet::new())),
    )
}

/// Prefer an authenticated declaration frame's access facts; a native callable without one retains its reference
/// spelling as the shared/exclusive authority it already had.
fn loaf_signature(callable: &bir::LoafCallable) -> Signature {
    let parameters = callable
        .declaration_frame()
        .filter(|frame| frame.params.len() == callable.parameters.len())
        .map(|frame| {
            frame
                .params
                .iter()
                .enumerate()
                .map(|(index, parameter)| SignatureParameter {
                    ty: callable.parameters[index].clone(),
                    access: parameter.input_access,
                })
                .collect()
        })
        .unwrap_or_else(|| {
            callable
                .parameters
                .iter()
                .map(|ty| SignatureParameter {
                    ty: ty.clone(),
                    access: incan_semantics_core::IncanInputAccess::Inferred,
                })
                .collect()
        });
    Signature {
        identity: callable.canonical.clone(),
        generic: !callable.type_parameters.is_empty(),
        arguments: callable.native_type_arguments.clone(),
        parameters,
    }
}

/// Project one body's parameter access independently from its executable statements. Anonymous frames supply no
/// named signature. Candidate inputs are shared only for this fixed-point hypothesis.
fn body_signature(body: &bir::Body, body_index: usize, candidates: &HashSet<(usize, usize)>) -> Option<Signature> {
    Some(Signature {
        identity: body.canonical.clone()?,
        generic: !body.type_parameters.is_empty(),
        arguments: Vec::new(),
        parameters: body
            .params
            .iter()
            .enumerate()
            .map(|(parameter_index, parameter)| SignatureParameter {
                ty: parameter.ty.clone(),
                access: if candidates.contains(&(body_index, parameter_index)) {
                    incan_semantics_core::IncanInputAccess::InferredShared
                } else {
                    parameter.input_access
                },
            })
            .collect(),
    })
}

/// Only native owners with retained declaration identity participate; source models and open types keep their ABI.
fn native_owner(ty: &IncanType, identities: &std::collections::BTreeMap<String, CanonicalSymbolId>) -> bool {
    let (name, checked_path) = match ty {
        IncanType::Named(name) => (name, false),
        IncanType::Generic { base: name, .. } | IncanType::RustInteropPath(name) => (name, true),
        _ => return false,
    };
    let declaration_name = name.rsplit("::").next().unwrap_or(name);
    identities
        .get(name)
        .or_else(|| {
            identities
                .values()
                .find(|identity| identity.declaration_name == declaration_name)
        })
        .is_some_and(|identity| {
            identity.kind == SemanticSourceTargetKind::RustItem && matches!(identity.origin, SymbolOrigin::RustCrate(_))
        })
        || checked_path && bir::rust_item_identity(name).is_some()
}

/// Borrow a shared native field binding or traversal cursor when every use stays read-only. A field binding starts
/// from an explicit projected loan; a cursor's later assignments must already return shared views of its owner.
pub(super) fn retain_cursors(body: &mut bir::Body, signatures: &[Signature]) {
    for index in 0..body.locals.len() {
        let local = body.locals[index].id;
        let owner = body.locals[index].ty.clone();
        if !matches!(body.locals[index].origin, bir::LocalOrigin::UserBinding)
            || !native_owner(&owner, &body.named_type_identities)
        {
            continue;
        }
        let mut writes = Vec::new();
        walk(&body.block.stmts, &mut |statement| {
            if let bir::StatementKind::Assign { place, rvalue } = &statement.kind
                && place.local_id() == Some(local)
            {
                writes.push((place.clone(), rvalue.clone()));
            }
        });
        let Some((_, bir::Rvalue::Use(bir::Operand::Place(initial)))) = writes.first() else {
            continue;
        };
        let source = initial.place.clone();
        let reference = IncanType::Ref(Box::new(owner.clone()));
        let field_view = writes.len() == 1
            && initial.fact == bir::OwnershipFact::Borrow
            && !source.projection.is_empty()
            && source.local_id().is_some();
        if (!field_view && writes.len() < 2) || writes.iter().any(|(place, _)| !place.projection.is_empty())
            || writes.iter().skip(1).any(|(_, value)| !matches!(value,
                bir::Rvalue::Use(bir::Operand::Place(read)) if whole_type(&read.place, &body.locals) == Some(&reference)))
        { continue; }
        let context = ReadContext {
            locals: &body.locals,
            signatures,
            local,
            owner: &owner,
            cursor: true,
        };
        if !readonly(&body.block.stmts, &context) || source_changes(&body.block.stmts, &source, local) {
            continue;
        }
        body.locals[index].ty = reference;
        walk_mut(&mut body.block.stmts, &mut |statement| {
            if let bir::StatementKind::Assign {
                place,
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
            } = &mut statement.kind
                && place.local_id() == Some(local)
            {
                read.fact = bir::OwnershipFact::Borrow;
                read.last_use = false;
            }
        });
    }
}

/// Refuse a view when the frame replaces or exclusively lends its original owner. Initializing the cursor itself
/// is exempt; the checker already admitted that original value read.
fn source_changes(statements: &[bir::Statement], source: &bir::Place, cursor: bir::LocalId) -> bool {
    let mut changed = false;
    walk(statements, &mut |statement| match &statement.kind {
        bir::StatementKind::Assign { place, .. } if place.root == source.root && place.local_id() != Some(cursor) => {
            changed = true
        }
        bir::StatementKind::Call { args, .. } => {
            changed |= args.iter().filter_map(bir::ArgumentElement::as_one).any(|argument| matches!(argument,
                bir::Operand::Place(read) if read.place.root == source.root && read.fact == bir::OwnershipFact::MutBorrow));
        }
        _ => {}
    });
    changed
}

/// Read the checked type of a whole reference binding; projected initialization keeps its checked destination type.
fn whole_type<'a>(place: &bir::Place, locals: &'a [bir::LocalDecl]) -> Option<&'a IncanType> {
    if !place.projection.is_empty() {
        return None;
    }
    locals.get(place.local_id()?.index()).map(|local| &local.ty)
}

/// Prove all uses in this frame while conservatively declining unsupported control flow and escaping values.
fn readonly(statements: &[bir::Statement], context: &ReadContext<'_>) -> bool {
    let mut accepted = true;
    walk(statements, &mut |statement| {
        accepted &= shared_statement(statement, context);
    });
    accepted
}

/// Admit a shared call or local reference binding; consuming, exclusive and escaping operations keep owned storage.
fn shared_statement(statement: &bir::Statement, context: &ReadContext<'_>) -> bool {
    use bir::StatementKind as S;
    match &statement.kind {
        S::Assign { place, rvalue } => {
            if place.local_id() == Some(context.local) {
                return context.cursor && place.projection.is_empty() && matches!(rvalue, bir::Rvalue::Use(_));
            }
            if let bir::Rvalue::Use(value) = rvalue
                && uses(value, context.local)
            {
                return scalar_projection(value, context.local)
                    || whole_type(place, context.locals) == Some(&IncanType::Ref(Box::new(context.owner.clone())));
            }
            shared_rvalue(rvalue, context.local)
        }
        S::Call {
            destination,
            callee,
            args,
            ..
        } => {
            if destination
                .as_ref()
                .is_some_and(|place| place.local_id() == Some(context.local))
            {
                return false;
            }
            args.iter().enumerate().all(|(index, argument)| {
                let Some(value) = argument.as_one() else { return false };
                !uses(value, context.local)
                    || matches!(value, bir::Operand::Place(read) if read.fact == bir::OwnershipFact::Borrow)
                    || call_parameter(callee, index, context.signatures).and_then(SignatureParameter::shared_owner)
                        == Some(context.owner)
            })
        }
        S::If { cond, .. } => !uses(cond, context.local),
        S::Loop { .. } | S::UnsafeRegion { .. } | S::Drop { .. } | S::Continue => true,
        S::Return { value } | S::Break { value } => value
            .as_ref()
            .is_none_or(|value| !uses(value, context.local) || scalar_projection(value, context.local)),
        S::Expr { value } => !uses(value, context.local) || scalar_projection(value, context.local),
        S::IterNext {
            destination, iterator, ..
        } => destination.local_id() != Some(context.local) && !uses(iterator, context.local),
        S::Assert { kind, message, .. } => {
            let value = match kind {
                bir::AssertionKind::Condition { cond } => cond,
                bir::AssertionKind::Pattern { scrutinee, .. } => scrutinee,
                bir::AssertionKind::Raises { call, .. } => call,
            };
            !uses(value, context.local) && message.as_ref().is_none_or(|value| !uses(value, context.local))
        }
        _ => false,
    }
}

/// Ordinary scalar work and formatting may coexist with a read loan; closure environments and aggregate ownership
/// are declined. A checked Copy field projection observes its scalar without copying the native owner.
fn shared_rvalue(value: &bir::Rvalue, local: bir::LocalId) -> bool {
    match value {
        bir::Rvalue::Use(value) | bir::Rvalue::UnaryOp(_, value) | bir::Rvalue::IsInstance { value, .. } => {
            !uses(value, local) || scalar_projection(value, local)
        }
        bir::Rvalue::BinaryOp(_, left, right) => {
            (!uses(left, local) || scalar_projection(left, local))
                && (!uses(right, local) || scalar_projection(right, local))
        }
        bir::Rvalue::Format(parts) => parts.iter().all(|part| match part {
            bir::FormatPart::Literal(_) => true,
            bir::FormatPart::Expr { operand, .. } => !uses(operand, local) || scalar_projection(operand, local),
        }),
        bir::Rvalue::Match { scrutinee, arms } => {
            !uses(scrutinee, local)
                && arms
                    .iter()
                    .all(|arm| arm.guard.as_ref().is_none_or(|guard| !uses(guard, local)) && !uses(&arm.result, local))
        }
        _ => false,
    }
}

/// A checked Copy read through a field observes that field rather than transferring its native owner.
fn scalar_projection(value: &bir::Operand, local: bir::LocalId) -> bool {
    matches!(value, bir::Operand::Place(read) if read.place.local_id() == Some(local)
        && !read.place.projection.is_empty() && read.fact == bir::OwnershipFact::Copy)
}

/// Detect a read rooted in this lexical local, independently of its diagnostic spelling.
fn uses(value: &bir::Operand, local: bir::LocalId) -> bool {
    matches!(value, bir::Operand::Place(read) if read.place.local_id() == Some(local))
}

/// Resolve a function or method slot only from the retained canonical target, instantiation and checked argument
/// binding. A method binding excludes its receiver, while its body signature keeps that receiver in slot zero.
fn call_parameter<'a>(
    callee: &bir::Callee,
    index: usize,
    signatures: &'a [Signature],
) -> Option<&'a SignatureParameter> {
    let (identity, arguments, slot) = match callee {
        bir::Callee::Function(bir::CallableTarget::Named(target)) => (
            target.canonical.as_ref()?,
            &target.type_args,
            target.binding.slot_arguments()?.get(index)?.slot,
        ),
        bir::Callee::Method(target) => {
            let slot = if index == 0 {
                0
            } else {
                target.binding.slot_arguments()?.get(index - 1)?.slot + 1
            };
            (target.canonical.as_ref()?, &target.type_args, slot)
        }
        _ => return None,
    };
    let mut exact = signatures
        .iter()
        .filter(|signature| signature.identity == *identity && signature.arguments == *arguments);
    if let Some(signature) = exact.next() {
        if exact.any(|other| other.parameters != signature.parameters) {
            return None;
        }
        return signature.parameters.get(slot);
    }
    let mut generic = signatures
        .iter()
        .filter(|signature| signature.identity == *identity && signature.generic);
    let signature = generic.next()?;
    if generic.any(|other| other.parameters != signature.parameters) {
        return None;
    }
    signature.parameters.get(slot)
}

/// Retain the callee's shared passing decision at each source call; the backend receives no inference task.
fn retain_call_loans(statement: &mut bir::Statement, signatures: &[Signature]) {
    if let bir::StatementKind::Call { callee, args, .. } = &mut statement.kind {
        for (index, argument) in args.iter_mut().enumerate() {
            if let bir::ArgumentElement::One(bir::Operand::Place(read)) = argument
                && call_parameter(callee, index, signatures)
                    .and_then(SignatureParameter::inferred_shared_owner)
                    .is_some()
            {
                read.fact = bir::OwnershipFact::Borrow;
                read.last_use = false;
            }
        }
    }
}

/// Traverse lexical control flow while leaving closure and generator frames independent.
fn walk(statements: &[bir::Statement], visit: &mut impl FnMut(&bir::Statement)) {
    for statement in statements {
        visit(statement);
        match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    walk(&arm.guard_stmts, visit);
                    walk(&arm.body_stmts, visit);
                }
            }
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                walk(&then_block.stmts, visit);
                if let Some(block) = else_block {
                    walk(&block.stmts, visit);
                }
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                walk(&body.stmts, visit)
            }
            _ => {}
        }
    }
}

/// Rewrite the same lexical statement surface without allowing a nested callable to share frame-local identities.
fn walk_mut(statements: &mut [bir::Statement], visit: &mut impl FnMut(&mut bir::Statement)) {
    walk_statements_mut(statements, false, visit);
}

/// Rewrite every call site of one body, including the calls inside its closure and generator bodies.
fn walk_calls_mut(statements: &mut [bir::Statement], visit: &mut impl FnMut(&mut bir::Statement)) {
    walk_statements_mut(statements, true, visit);
}

/// The one mutable statement walk: lexical control flow always, closure and generator bodies when `enter_callables`.
fn walk_statements_mut(
    statements: &mut [bir::Statement],
    enter_callables: bool,
    visit: &mut impl FnMut(&mut bir::Statement),
) {
    for statement in statements {
        visit(statement);
        match &mut statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    walk_statements_mut(&mut arm.guard_stmts, enter_callables, visit);
                    walk_statements_mut(&mut arm.body_stmts, enter_callables, visit);
                }
            }
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Closure { body, .. },
                ..
            } if enter_callables => walk_statements_mut(&mut body.stmts, enter_callables, visit),
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Generator { body, .. },
                ..
            } if enter_callables => walk_statements_mut(&mut body.stmts, enter_callables, visit),
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                walk_statements_mut(&mut then_block.stmts, enter_callables, visit);
                if let Some(block) = else_block {
                    walk_statements_mut(&mut block.stmts, enter_callables, visit);
                }
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                walk_statements_mut(&mut body.stmts, enter_callables, visit)
            }
            _ => {}
        }
    }
}
