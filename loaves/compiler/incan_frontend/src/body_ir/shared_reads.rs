//! Retain nonescaping native read loans before the executable ABI is published. Owned snapshots, mutable uses,
//! callable values and public signatures keep their original storage contracts.

use super::{CanonicalSymbolId, IncanType, SemanticSourceTargetKind, ast, bir};
use incan_semantics_core::SymbolOrigin;

/// One canonical callable's already checked closed storage signature.
struct Signature {
    identity: CanonicalSymbolId,
    arguments: Vec<IncanType>,
    parameters: Vec<IncanType>,
}

/// A read-only proof is local to one frame and never grants an escaping or exclusive loan.
struct ReadContext<'a> {
    locals: &'a [bir::LocalDecl],
    signatures: &'a [Signature],
    local: bir::LocalId,
    owner: &'a IncanType,
    cursor: bool,
}

/// Retain native cursors and close private helper loans over canonical calls. Each cursor must receive a borrowed
/// native value after initialization and use only shared operations; replacing its source owner preserves a snapshot.
pub(super) fn retain(module: &mut bir::BodyIrModule, program: &ast::Program) {
    let mut signatures = signatures(module);
    for body in &mut module.bodies {
        retain_cursors(body, &signatures);
    }
    let private = program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let ast::Declaration::Function(function) = &declaration.node else {
                return None;
            };
            (function.visibility == ast::Visibility::Private && function.type_params.is_empty() && !function.is_async())
                .then_some(super::hir_span(declaration.span))
        })
        .collect::<Vec<_>>();
    let escaped = escaped_functions(module);
    loop {
        let mut changed = false;
        for body in &mut module.bodies {
            if !private.contains(&body.span)
                || body.canonical.as_ref().is_some_and(|id| escaped.contains(id))
                || !matches!(body.return_type, IncanType::Primitive(_))
                || !body.return_type.abi_v0_facts().ownership.is_trivially_copy()
                || body.extern_delegation.is_some()
                || !body.native_attributes.is_empty()
            {
                continue;
            }
            for parameter in &mut body.params {
                if parameter.mutable || !native_owner(&parameter.ty, &body.named_type_identities) {
                    continue;
                }
                let context = ReadContext {
                    locals: &body.locals,
                    signatures: &signatures,
                    local: parameter.local,
                    owner: &parameter.ty,
                    cursor: false,
                };
                if readonly(&body.block.stmts, &context) {
                    parameter.ty = IncanType::Ref(Box::new(parameter.ty.clone()));
                    body.locals[parameter.local.index()].ty = parameter.ty.clone();
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
        signatures = self::signatures(module);
    }
    for body in &mut module.bodies {
        walk_mut(&mut body.block.stmts, &mut |statement| {
            retain_call_loans(statement, &signatures)
        });
    }
}

/// Combine native selections and concrete local bodies without deriving a signature from a name.
fn signatures(module: &bir::BodyIrModule) -> Vec<Signature> {
    module
        .loaf_callables
        .iter()
        .map(|callable| Signature {
            identity: callable.canonical.clone(),
            arguments: callable.native_type_arguments.clone(),
            parameters: callable.parameters.clone(),
        })
        .chain(module.bodies.iter().filter_map(|body| {
            Some(Signature {
                identity: body.canonical.clone()?,
                arguments: Vec::new(),
                parameters: body.params.iter().map(|parameter| parameter.ty.clone()).collect(),
            })
        }))
        .collect()
}

/// Only native owners with retained declaration identity participate; source models and open types keep their ABI.
fn native_owner(ty: &IncanType, identities: &std::collections::BTreeMap<String, CanonicalSymbolId>) -> bool {
    let IncanType::Named(name) = ty else { return false };
    identities.get(name).is_some_and(|identity| {
        identity.kind == SemanticSourceTargetKind::RustItem && matches!(identity.origin, SymbolOrigin::RustCrate(_))
    })
}

/// A private function exposed as a callable retains the callable's owned argument contract.
fn escaped_functions(module: &bir::BodyIrModule) -> Vec<CanonicalSymbolId> {
    let mut escaped = Vec::new();
    for body in &module.bodies {
        walk(&body.block.stmts, &mut |statement| {
            if let bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Closure { body, .. },
                ..
            } = &statement.kind
                && body.function_item
            {
                walk(&body.stmts, &mut |statement| {
                    if let bir::StatementKind::Call {
                        callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                        ..
                    } = &statement.kind
                        && let Some(identity) = &target.canonical
                    {
                        escaped.push(identity.clone());
                    }
                });
            }
        });
    }
    escaped
}

/// Borrow a shared native field binding or traversal cursor when every use stays read-only. A field binding starts
/// from an explicit projected loan; a cursor's later assignments must already return shared views of its owner.
fn retain_cursors(body: &mut bir::Body, signatures: &[Signature]) {
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
                    || call_parameter(callee, index, context.signatures)
                        .is_some_and(|ty| matches!(ty, IncanType::Ref(inner) if inner.as_ref() == context.owner))
            })
        }
        S::If { cond, .. } => !uses(cond, context.local),
        S::Loop { .. } | S::UnsafeRegion { .. } | S::Drop { .. } | S::Continue => true,
        S::Return { value } | S::Break { value } => value.as_ref().is_none_or(|value| !uses(value, context.local)),
        S::Expr { value } => !uses(value, context.local),
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

/// Resolve a slot only from the retained canonical target, instantiation and checked argument binding.
fn call_parameter<'a>(callee: &bir::Callee, index: usize, signatures: &'a [Signature]) -> Option<&'a IncanType> {
    let bir::Callee::Function(bir::CallableTarget::Named(target)) = callee else {
        return None;
    };
    let identity = target.canonical.as_ref()?;
    let slot = target.binding.slot_arguments()?.get(index)?.slot;
    let mut selected = signatures
        .iter()
        .filter(|signature| signature.identity == *identity && signature.arguments == target.type_args);
    let signature = selected.next()?;
    if selected.any(|other| other.parameters != signature.parameters) {
        return None;
    }
    signature.parameters.get(slot)
}

/// Retain the callee's shared passing decision at each source call; the backend receives no inference task.
fn retain_call_loans(statement: &mut bir::Statement, signatures: &[Signature]) {
    if let bir::StatementKind::Call { callee, args, .. } = &mut statement.kind {
        for (index, argument) in args.iter_mut().enumerate() {
            if let bir::ArgumentElement::One(bir::Operand::Place(read)) = argument
                && matches!(call_parameter(callee, index, signatures), Some(IncanType::Ref(_)))
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
    for statement in statements {
        visit(statement);
        match &mut statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    walk_mut(&mut arm.guard_stmts, visit);
                    walk_mut(&mut arm.body_stmts, visit);
                }
            }
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                walk_mut(&mut then_block.stmts, visit);
                if let Some(block) = else_block {
                    walk_mut(&mut block.stmts, visit);
                }
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                walk_mut(&mut body.stmts, visit)
            }
            _ => {}
        }
    }
}
