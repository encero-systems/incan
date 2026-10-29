//! Retain nonescaping native read loans before the executable ABI is published. Owned snapshots, mutable uses,
//! callable values and public signatures keep their original storage contracts.

use std::collections::HashSet;

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
        retain_consumed_lists(body);
        retain_iteration_items(body);
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
        .collect::<HashSet<_>>();
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
    // A callee's signature is module-level, so a call inside a closure or generator body takes the same loans.
    for body in &mut module.bodies {
        walk_calls_mut(&mut body.block.stmts, &mut |statement| {
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
fn escaped_functions(module: &bir::BodyIrModule) -> HashSet<CanonicalSymbolId> {
    let mut escaped = HashSet::new();
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
                        escaped.insert(identity.clone());
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

/// Bind a builtin loop's item by reference when the loop reads a list it borrows (#2426).
///
/// Body IR initializes a builtin list iterator with a shared borrow unless the loop takes the list's items or changes
/// them in place. Such a loop reads each item where it stays, so an item of a model, class or text type is a
/// shared reference to its element rather than a copy of it. A read that hands the item on by value clones it there,
/// at the use, never once per step. The item must be written only by its `IterNext`.
fn retain_iteration_items(body: &mut bir::Body) {
    let mut iterators = Vec::new();
    walk(&body.block.stmts, &mut |statement| {
        if let bir::StatementKind::Assign {
            place,
            rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
        } = &statement.kind
            && read.fact == bir::OwnershipFact::Borrow
            && let Some(local) = place.local_id()
            && place.projection.is_empty()
        {
            iterators.push(local);
        }
    });
    let items: Vec<_> = single_polls(&body.block.stmts)
        .into_iter()
        .filter(|(iterator, _)| iterators.contains(iterator) && is_list(&body.locals[iterator.index()].ty))
        .map(|(_, item)| item)
        .collect();
    for item in items {
        let ty = body.locals[item.index()].ty.clone();
        if !borrowable_item(&ty, &body.named_type_identities) || item_writes(&body.block.stmts, item) != 1
            || captured(&body.block.stmts, item)
            || matched(&body.block.stmts, item)
        {
            continue;
        }
        // An item lent exclusively, to a `mut` parameter say, keeps its own copy (#1561).
        let mut statements = body.block.stmts.clone();
        let mut exclusive = false;
        walk_mut(&mut statements, &mut |statement| {
            let mut rewrite = |operand: &mut bir::Operand| exclusive |= clone_item_move(operand, item);
            statement_places_mut(statement, &mut |place| place_indices_mut(place, &mut rewrite));
            statement_operands_mut(statement, &mut rewrite);
        });
        if !exclusive {
            body.locals[item.index()].ty = IncanType::Ref(Box::new(ty));
            body.block.stmts = statements;
        }
    }
}

/// A builtin list, as Body IR spells its checked type.
fn is_list(ty: &IncanType) -> bool {
    matches!(ty, IncanType::Generic { base, args } if base == "List" && args.len() == 1)
}

/// An item type that owns storage and has a shared reference the lowering reads in place: a source model, class or
/// enum, a Rust item, text, a list, or a tuple holding any of these.
fn borrowable_item(ty: &IncanType, identities: &std::collections::BTreeMap<String, CanonicalSymbolId>) -> bool {
    match ty {
        IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Str) => true,
        IncanType::Named(name) => identities.get(name).is_some_and(|identity| {
            matches!(
                identity.kind,
                SemanticSourceTargetKind::Model
                    | SemanticSourceTargetKind::Class
                    | SemanticSourceTargetKind::Enum
                    | SemanticSourceTargetKind::RustItem
            )
        }),
        IncanType::Tuple(elements) => elements.iter().any(|element| borrowable_item(element, identities)),
        _ => is_list(ty),
    }
}

/// A closure or generator capturing the item keeps it owned: its environment holds values, not loans.
fn captured(statements: &[bir::Statement], item: bir::LocalId) -> bool {
    let mut found = false;
    walk(statements, &mut |statement| {
        if let bir::StatementKind::Assign {
            rvalue: bir::Rvalue::Closure { captured_operands, .. } | bir::Rvalue::Generator { captured_operands, .. },
            ..
        } = &statement.kind
        {
            found |= captured_operands.iter().any(|operand| uses(operand, item));
        }
    });
    found
}

/// An item matched by a pattern that binds a payload keeps its copy: payload bindings move out of owned values.
fn matched(statements: &[bir::Statement], item: bir::LocalId) -> bool {
    let mut found = false;
    walk(statements, &mut |statement| match &statement.kind {
        bir::StatementKind::Assign {
            rvalue: bir::Rvalue::Match { scrutinee, arms },
            ..
        } => found |= uses(scrutinee, item) && arms.iter().any(|arm| binds(&arm.pattern)),
        bir::StatementKind::Assert {
            kind: bir::AssertionKind::Pattern { scrutinee, pattern },
            ..
        } => found |= uses(scrutinee, item) && binds(pattern),
        _ => {}
    });
    found
}

/// Count the statements that write `item` as a whole or in part, its `IterNext` included.
fn item_writes(statements: &[bir::Statement], item: bir::LocalId) -> usize {
    let mut writes = 0;
    walk(statements, &mut |statement| {
        let place = match &statement.kind {
            bir::StatementKind::Assign { place, .. } | bir::StatementKind::IterNext { destination: place, .. } => {
                Some(place)
            }
            bir::StatementKind::Call { destination, .. } | bir::StatementKind::Await { destination, .. } => {
                destination.as_ref()
            }
            bir::StatementKind::TryPropagate { destination, .. } => Some(destination),
            _ => None,
        };
        writes += usize::from(place.is_some_and(|place| place.local_id() == Some(item)));
    });
    writes
}

/// Visit every operand one statement reads directly; nested blocks are reached by the statement walk.
fn statement_operands_mut(statement: &mut bir::Statement, visit: &mut impl FnMut(&mut bir::Operand)) {
    use bir::StatementKind as S;
    match &mut statement.kind {
        S::Assign { rvalue, .. } => rvalue_operands_mut(rvalue, visit),
        S::Call { args, .. } => arguments_mut(args, visit),
        S::If { cond, .. } => visit(cond),
        S::Break { value } | S::Return { value } => value.iter_mut().for_each(|value| visit(value)),
        S::Await { awaited, .. } => visit(awaited),
        S::Race { arms, .. } => arms.iter_mut().for_each(|arm| {
            visit(&mut arm.awaitable);
            visit(&mut arm.result);
        }),
        S::Yield { value } | S::Expr { value } => visit(value),
        S::Assert { kind, message, .. } => {
            match kind {
                bir::AssertionKind::Condition { cond } => visit(cond),
                bir::AssertionKind::Pattern { scrutinee, .. } => visit(scrutinee),
                bir::AssertionKind::Raises { call, .. } => visit(call),
            }
            message.iter_mut().for_each(|value| visit(value));
        }
        S::TryPropagate { operand, .. } => visit(operand),
        S::IterNext { iterator, .. } => visit(iterator),
        S::UnsafeRegion { .. } | S::Drop { .. } | S::Loop { .. } | S::Continue | S::Unsupported { .. } => {}
    }
}

/// Visit the operands of one right-hand side, closure and generator captures included.
fn rvalue_operands_mut(rvalue: &mut bir::Rvalue, visit: &mut impl FnMut(&mut bir::Operand)) {
    use bir::Rvalue as R;
    match rvalue {
        R::Use(value) | R::UnaryOp(_, value) | R::IsInstance { value, .. } => visit(value),
        R::BinaryOp(_, left, right) => {
            visit(left);
            visit(right);
        }
        R::Aggregate(_, elements) => arguments_mut(elements, visit),
        R::Dict(entries) => entries.iter_mut().for_each(|entry| match entry {
            bir::DictEntry::Pair(key, value) => {
                visit(key);
                visit(value);
            }
            bir::DictEntry::Spread(spread) => visit(&mut spread.source),
        }),
        R::ResultVariant(variant) => visit(&mut variant.payload),
        R::Format(parts) => parts.iter_mut().for_each(|part| {
            if let bir::FormatPart::Expr { operand, .. } = part {
                visit(operand);
            }
        }),
        R::Closure { captured_operands, .. } => captured_operands.iter_mut().for_each(|value| visit(value)),
        R::Generator {
            source,
            captured_operands,
            ..
        } => {
            visit(source);
            captured_operands.iter_mut().for_each(|value| visit(value));
        }
        R::Match { scrutinee, arms } => {
            visit(scrutinee);
            for arm in arms {
                arm.guard.iter_mut().for_each(|guard| visit(guard));
                visit(&mut arm.result);
            }
        }
        R::ValueEnumVariant(_) | R::FieldlessEnumVariant(_) => {}
    }
}

/// Visit positional, named and spread call or aggregate arguments.
fn arguments_mut(arguments: &mut [bir::ArgumentElement], visit: &mut impl FnMut(&mut bir::Operand)) {
    for argument in arguments {
        match argument {
            bir::ArgumentElement::One(operand) | bir::ArgumentElement::Named { operand, .. } => visit(operand),
            bir::ArgumentElement::Spread(spread) => visit(&mut spread.source),
        }
    }
}

/// Turn a move of a borrowed loop item into a clone at that use, including inside index operands; report an exclusive
/// loan of the item.
fn clone_item_move(operand: &mut bir::Operand, item: bir::LocalId) -> bool {
    let bir::Operand::Place(read) = operand else { return false };
    let mut exclusive = false;
    place_indices_mut(&mut read.place, &mut |index| exclusive |= clone_item_move(index, item));
    if read.place.local_id() != Some(item) {
        return exclusive;
    }
    if read.fact == bir::OwnershipFact::Move {
        read.fact = bir::OwnershipFact::Clone;
        read.last_use = false;
    }
    exclusive || read.fact == bir::OwnershipFact::MutBorrow
}

/// Visit the index operands of one place's projection.
fn place_indices_mut(place: &mut bir::Place, visit: &mut impl FnMut(&mut bir::Operand)) {
    for element in &mut place.projection {
        if let bir::PlaceElem::Index(index) = element {
            visit(index);
        }
    }
}

/// Visit the places one statement writes.
fn statement_places_mut(statement: &mut bir::Statement, visit: &mut impl FnMut(&mut bir::Place)) {
    use bir::StatementKind as S;
    match &mut statement.kind {
        S::Assign { place, .. } | S::IterNext { destination: place, .. } | S::TryPropagate { destination: place, .. } => {
            visit(place)
        }
        S::Call { destination, .. } | S::Await { destination, .. } | S::Race { destination, .. } => {
            destination.iter_mut().for_each(|place| visit(place))
        }
        _ => {}
    }
}

/// Move an owned local list into its builtin loop when nothing reads the list again, so the loop owns the items (#2426).
///
/// The loop must run once: it sits in the body's own statements or their branches, never inside another loop or a
/// match arm. No later statement may name the list, and no closure or generator may hold it. Such a loop's items are
/// owned values, so a body that hands one on moves it instead of cloning it.
fn retain_consumed_lists(body: &mut bir::Body) {
    let mut candidates = Vec::new();
    once_statements(&body.block.stmts, &mut |position, statement| {
        if let bir::StatementKind::Assign {
            place,
            rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
        } = &statement.kind
            && read.fact == bir::OwnershipFact::Borrow
            && read.place.projection.is_empty()
            && place.projection.is_empty()
            && let (Some(iterator), Some(list)) = (place.local_id(), read.place.local_id())
            && is_list(&body.locals[list.index()].ty)
            && matches!(body.locals[list.index()].origin, bir::LocalOrigin::UserBinding)
            && matches!(body.locals[iterator.index()].origin, bir::LocalOrigin::Temporary)
        {
            candidates.push((position, iterator, list));
        }
    });
    for (position, iterator, list) in candidates {
        if !polled_builtin(&body.block.stmts, iterator)
            || captured(&body.block.stmts, list)
            || named_after(&mut body.block.stmts.clone(), position, list)
        {
            continue;
        }
        walk_mut(&mut body.block.stmts, &mut |statement| {
            if let bir::StatementKind::Assign {
                place,
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
            } = &mut statement.kind
                && place.local_id() == Some(iterator)
                && read.place.local_id() == Some(list)
            {
                read.fact = bir::OwnershipFact::Move;
                read.last_use = true;
            }
        });
    }
}

/// Visit, in source order with their walk positions, the statements that run at most once per call.
fn once_statements(statements: &[bir::Statement], visit: &mut impl FnMut(usize, &bir::Statement)) {
    let mut position = 0;
    let mut repeated = Vec::new();
    walk(statements, &mut |statement| {
        repeated.retain(|end: &usize| position < *end);
        if repeated.is_empty() {
            visit(position, statement);
        }
        let nested = match &statement.kind {
            bir::StatementKind::Loop { body } => Some(walked_len(&body.stmts)),
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } => Some(arms.iter().map(|arm| walked_len(&arm.guard_stmts) + walked_len(&arm.body_stmts)).sum()),
            _ => None,
        };
        if let Some(length) = nested {
            repeated.push(position + 1 + length);
        }
        position += 1;
    });
}

/// Count the statements the lexical walk visits in a block, nested blocks included.
fn walked_len(statements: &[bir::Statement]) -> usize {
    let mut count = 0;
    walk(statements, &mut |_| count += 1);
    count
}

/// Report whether a loop polls this iterator alone.
fn polled_builtin(statements: &[bir::Statement], iterator: bir::LocalId) -> bool {
    single_polls(statements).iter().any(|(polled, _)| *polled == iterator)
}

/// The iterator and whole item local of each loop that polls exactly one builtin iterator. A loop polling several, such
/// as `zip`, keeps its sources' own iteration contract.
fn single_polls(statements: &[bir::Statement]) -> Vec<(bir::LocalId, bir::LocalId)> {
    let mut polls = Vec::new();
    walk(statements, &mut |statement| {
        let bir::StatementKind::Loop { body } = &statement.kind else { return };
        let direct: Vec<_> = body
            .stmts
            .iter()
            .filter_map(|statement| match &statement.kind {
                bir::StatementKind::IterNext {
                    destination,
                    iterator: bir::Operand::Place(read),
                    protocol: bir::IterProtocol::Builtin,
                } if read.place.projection.is_empty() && destination.projection.is_empty() => {
                    Some((read.place.local_id()?, destination.local_id()?))
                }
                _ => None,
            })
            .collect();
        if let [poll] = direct.as_slice() {
            polls.push(*poll);
        }
    });
    polls
}

/// Report whether any statement after walk position `position` reads or writes `list`, drops excepted.
fn named_after(statements: &mut [bir::Statement], position: usize, list: bir::LocalId) -> bool {
    let mut current = 0;
    let mut named = false;
    walk_mut(statements, &mut |statement| {
        if current > position && !matches!(statement.kind, bir::StatementKind::Drop { .. }) {
            let mut found = false;
            statement_places_mut(statement, &mut |place| {
                found |= place.local_id() == Some(list);
                place_indices_mut(place, &mut |operand| found |= mentions_local(operand, list));
            });
            statement_operands_mut(statement, &mut |operand| found |= mentions_local(operand, list));
            named |= found;
        }
        current += 1;
    });
    named
}

/// Report whether an operand reads `local`, directly or through an index operand.
fn mentions_local(operand: &mut bir::Operand, local: bir::LocalId) -> bool {
    let bir::Operand::Place(read) = operand else { return false };
    let mut found = read.place.local_id() == Some(local);
    place_indices_mut(&mut read.place, &mut |index| found |= mentions_local(index, local));
    found
}

/// Report whether a pattern binds a name anywhere inside it.
fn binds(pattern: &bir::Pattern) -> bool {
    use bir::Pattern as P;
    match pattern {
        P::Var(_) => true,
        P::Tuple(fields) | P::Result { fields, .. } | P::Enum { fields, .. } | P::Or(fields) => fields.iter().any(binds),
        P::UnionMember { fields, .. } => fields.iter().any(binds),
        P::Struct { fields, .. } | P::Nominal { fields, .. } => fields.iter().any(|(_, field)| binds(field)),
        P::Wildcard | P::Literal(_) | P::FieldlessEnumVariant(_) => false,
    }
}
