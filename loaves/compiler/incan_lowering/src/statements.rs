//! Lower statements and structured control flow: blocks, returns, loop exits, races, branches, and loops.

use incan_mir_plan_native::{
    Constant, Operand, OperandKind, PlanType, Projection, RvalueKind, SourceSpan, Statement, StatementKind,
    TerminatorKind, assign as plan_assign, place as plan_place,
};
use incan_semantics_core::IncanInputAccess;
use incan_semantics_core::body_ir::{
    Block, BodyIrModule, Operand as SourceOperand, OwnershipFact, Statement as SourceStatement,
    StatementKind as SourceStatementKind,
};

use crate::assertions::lower_assertion;
use crate::assignments::lower_assignment;
use crate::builtin_calls::{coerce_numeric_operand, validate_exact_float};
use crate::calls::lower_call;
use crate::carrier_operands::boundary_operand;
use crate::checked_values::{builder_location, scalar_place};
use crate::closures::callable_copy_type;
use crate::explicit_arguments::{explicit_place_argument, reference_contract};
use crate::formatting::display_operand;
use crate::frame::{
    Builder, drop_all_owned, drop_owned, finish_fallthrough, forget_owned, literal, place_type, read_place,
    track_rvalue_moves,
};
use crate::list_iteration::list_next;
use crate::list_values::value_operand;
use crate::loop_exits::exits_to;
use crate::matches::join_branch_owners;
use crate::operand_borrows::lower_discarded_operand;
use crate::pending::untranslated;
use crate::result_carriers::lower_try_propagation;
use crate::tuples::tuple_has_owners;

/// Identify nominal, generator, zip, and tuple values whose owned payloads require transfer or destruction.
pub fn is_nominal_type(ty: &PlanType) -> bool {
    match ty {
        PlanType::ValidationError
        | PlanType::Enum(..)
        | PlanType::Model(..)
        | PlanType::ZipIterator(..)
        | PlanType::Generator(..) => true,
        PlanType::Tuple(_) => tuple_has_owners(ty),
        _ => false,
    }
}

/// Text and source nominals compare only through runtime operations, never through a native binary operator.
pub fn is_text_or_nominal(ty: &PlanType) -> bool {
    matches!(
        ty,
        PlanType::String | PlanType::StringRef | PlanType::StrRef | PlanType::Model(..) | PlanType::Enum(..)
    )
}

/// Identify an environment owner, which transfers and drops separately from copied callable references.
pub fn is_owned_callable(ty: &PlanType) -> bool {
    matches!(ty, PlanType::Closure(_) | PlanType::BorrowingClosure(_))
}

/// Release loop-local owners on a backedge, requiring enclosing storage to remain initialized for repetition.
pub fn drop_loop_owners(builder: &mut Builder, span: &SourceSpan) -> Result<(), String> {
    let incoming = builder.loop_owners.last().cloned().unwrap_or_default();
    for owner in &incoming {
        if !builder.owned.contains(owner) {
            return Err("unsupported Body IR loop changes incoming formatting ownership".into());
        }
    }
    drop_loop_local_owners(builder, span);
    Ok(())
}

/// Release only the exited iteration's local storage, retaining its result and all surviving enclosing owners.
fn drop_loop_local_owners(builder: &mut Builder, span: &SourceSpan) {
    let incoming = builder.loop_owners.last().cloned().unwrap_or_default();
    let retained = builder.owned.clone();
    for owner in retained.iter().rev() {
        if !incoming.contains(owner) && loop_local_owner(builder, *owner) {
            drop_owned(builder, *owner, span);
        }
    }
}

/// Use Body IR's lexical scope tree to distinguish loop-local storage from values declared outside the loop.
fn loop_local_owner(builder: &Builder, local: i64) -> bool {
    let Some(&(_, scope)) = builder.scoped_locals.iter().find(|(index, _)| *index == local) else {
        // Native temporaries have no source scope and belong to the current evaluation.
        return true;
    };
    let loop_scope = builder.loop_scopes.last().copied().unwrap_or(-1);
    let mut current = scope;
    for _ in 0..=builder.scope_parents.len() {
        if current == loop_scope {
            return true;
        }
        let mut parent = -1;
        for (enclosing_scope, enclosing) in &builder.scope_parents {
            if *enclosing_scope == current {
                parent = *enclosing;
            }
        }
        if parent < 0 {
            return false;
        }
        current = parent;
    }
    true
}

/// Validate source acknowledgments, retain refusal locations, and stop after termination.
pub fn lower_block(module: &BodyIrModule, builder: &mut Builder, block: &Block) -> Result<(), String> {
    // Receipts are minted by the frame's own source module, which for an imported body is not the entry module.
    if !block.unsafe_acknowledgments_valid(&builder.module_identity) {
        return Err("unsupported Body IR invalid unsafe acknowledgment receipt or enclosing scope".into());
    }
    // Borrow each statement: a copied statement would copy every block nested inside it.
    for statement in &block.stmts {
        if let Err(reason) = lower_statement(module, builder, statement) {
            let site = builder_location(builder, &statement.span);
            return Err(format!("{reason} at {}:{}:{}", site.file, site.line, site.column));
        }
        if builder.closed[builder.current] {
            return Ok(());
        }
    }
    Ok(())
}

/// Find the step's later clause that polls this iterator local, or -1 outside a step or for any other local.
fn later_clause_index(builder: &Builder, iterator: i64) -> i64 {
    if iterator < 0 {
        return -1;
    }
    builder
        .step_clauses
        .iter()
        .position(|clause| clause.0 == iterator)
        .map_or(-1, |index| index as i64)
}

/// Read a whole Body IR local's index, or -1 for a projection or a global.
fn whole_local(place: &incan_semantics_core::body_ir::Place) -> i64 {
    if !place.projection.is_empty() {
        return -1;
    }
    place.local_id().map_or(-1, |local| local.index() as i64)
}

/// Preserve statement order and refuse constructs outside the supported slice.
pub fn lower_statement(
    module: &BodyIrModule,
    builder: &mut Builder,
    statement: &SourceStatement,
) -> Result<(), String> {
    let span = match &builder.anchor {
        Some(site) => site.clone(),
        None => builder_location(builder, &statement.span),
    };
    match &statement.kind {
        SourceStatementKind::Assign { place, rvalue } => {
            // A later clause's iterator is never materialized: the clause's own chain draws from the source.
            if later_clause_index(builder, whole_local(place)) >= 0 {
                return Ok(());
            }
            lower_assignment(module, builder, place, rvalue, &span)
        }
        SourceStatementKind::UnsafeRegion { acknowledgment, body } => {
            if !acknowledgment.matches_region(&builder.module_identity, &statement.span, body.scope) {
                return Err("unsupported Body IR invalid unsafe acknowledgment receipt".into());
            }
            lower_block(module, builder, body)
        }
        SourceStatementKind::If {
            cond,
            then_block,
            else_block,
        } => lower_if(module, builder, cond, then_block, else_block.as_ref(), &span),
        SourceStatementKind::Loop { body } => {
            if !builder.step_clauses.is_empty() {
                return Err(untranslated("closure_lifting::lower_later_clause"));
            }
            lower_loop(module, builder, body, &span)
        }
        SourceStatementKind::Return { value } => lower_return(builder, value.as_ref(), &span),
        SourceStatementKind::Break { value } => match value {
            Some(_) => Err("unsupported Body IR value-producing Break".into()),
            None => lower_loop_exit(builder, false, &span),
        },
        SourceStatementKind::Continue => lower_loop_exit(builder, true, &span),
        SourceStatementKind::Drop { local } => {
            let index = local.index() as i64 + 1 + builder.local_offset;
            drop_owned(builder, index, &span);
            // Parameters stay in caller-provided storage for the whole invocation, even after their value drops.
            if index > builder.parameter_count {
                builder.emit(Statement {
                    kind: StatementKind::StorageDead(index),
                    span: span.clone(),
                });
            }
            Ok(())
        }
        // An expression statement only needs its operand to be valid; its value is discarded.
        SourceStatementKind::Expr { value } => lower_discarded_operand(builder, value, &span),
        SourceStatementKind::Call { .. } => lower_call(module, builder, statement, &span),
        SourceStatementKind::Await { .. } => Err(untranslated("async_::await_future")),
        SourceStatementKind::Race { .. } => Err(untranslated("statements::lower_native_race")),
        SourceStatementKind::Yield { .. } => Err(untranslated("statements::lower_yield")),
        SourceStatementKind::Assert { kind, message, .. } => lower_assertion(builder, kind, message.as_ref(), &span),
        SourceStatementKind::TryPropagate {
            destination,
            operand,
            error_routing,
        } => lower_try_propagation(builder, destination, operand, error_routing, &span),
        SourceStatementKind::IterNext {
            destination,
            iterator,
            protocol,
        } => list_next(builder, destination, iterator, protocol, &span),
        SourceStatementKind::Unsupported { description } => {
            Err(format!("unsupported Body IR Unsupported: {description}"))
        }
    }
}

/// Store the result in the return slot, drop every live owner, and return.
fn lower_return(builder: &mut Builder, value: Option<&SourceOperand>, span: &SourceSpan) -> Result<(), String> {
    let result = match value {
        Some(operand) => return_operand(builder, operand, span)?,
        None => literal(Constant::Unit, span),
    };
    finish_return(builder, result, span)
}

/// Materialize a returned value for the return slot's type, taking ownership of an owned result.
pub fn return_operand(builder: &mut Builder, operand: &SourceOperand, span: &SourceSpan) -> Result<Operand, String> {
    let return_type = builder.locals[0].ty.clone();
    if builder.returned_view != IncanInputAccess::Inferred {
        let access = builder.returned_view;
        return view_operand(builder, operand, &return_type, access, span);
    }
    if return_type == PlanType::String {
        let text = display_operand(builder, operand, span)?;
        forget_owned(builder, text.local);
        return Ok(read_place(text, true, span));
    }
    let result = boundary_operand(builder, operand, &return_type, span)?;
    track_rvalue_moves(builder, &RvalueKind::Use(result.clone()));
    Ok(result)
}

/// The one loan of a view's source place, for a direct view's return and a carried view's payload alike.
fn view_operand(
    builder: &mut Builder,
    operand: &SourceOperand,
    expected: &PlanType,
    access: IncanInputAccess,
    span: &SourceSpan,
) -> Result<Operand, String> {
    match operand {
        SourceOperand::Place(read) => {
            let source = scalar_place(builder, &read.place, span)?;
            explicit_place_argument(builder, source, read.fact, expected, access, span)
        }
        _ => Err("unsupported Body IR returned view without a source place".into()),
    }
}

/// Store a materialized result in the return slot, drop every live owner, and return.
pub fn finish_return(builder: &mut Builder, value: Operand, span: &SourceSpan) -> Result<(), String> {
    let return_type = builder.locals[0].ty.clone();
    let result = coerce_numeric_operand(builder, value, &return_type, span)?;
    if callable_copy_type(&return_type) {
        return Err(untranslated("closure_lifting::assign_callable"));
    }
    builder.emit(plan_assign(plan_place(0, span), RvalueKind::Use(result), span));
    validate_exact_float(builder, &plan_place(0, span), span);
    drop_all_owned(builder, span);
    builder.finish(TerminatorKind::Return, span);
    Ok(())
}

/// Clean the innermost iteration before an exit or backedge, requiring incoming owners only for repetition.
fn lower_loop_exit(builder: &mut Builder, repeat: bool, span: &SourceSpan) -> Result<(), String> {
    let Some(&(header, after)) = builder.loops.last() else {
        return Err("Body IR Break or Continue outside Loop".into());
    };
    if repeat {
        drop_loop_owners(builder, span)?;
        builder.finish(TerminatorKind::Goto(header), span);
        return Ok(());
    }
    drop_loop_local_owners(builder, span);
    let end = builder.current as i64;
    let owners = builder.owned.clone();
    if let Some(exits) = builder.loop_exit_blocks.last_mut() {
        exits.push(end);
    }
    if let Some(exit_owners) = builder.loop_exit_owners.last_mut() {
        exit_owners.push(owners);
    }
    builder.finish(TerminatorKind::Goto(after), span);
    Ok(())
}

/// Lower a proven constant branch directly; otherwise join the independently lowered branches and their owners.
fn lower_if(
    module: &BodyIrModule,
    builder: &mut Builder,
    condition: &SourceOperand,
    then_body: &Block,
    else_body: Option<&Block>,
    span: &SourceSpan,
) -> Result<(), String> {
    // `SwitchBool` takes its false target first, then its true target.
    let condition_value = value_operand(builder, condition, span)?;
    if let Some(selected) = constant_boolean(builder, &condition_value) {
        if selected {
            return lower_block(module, builder, then_body);
        }
        return match else_body {
            Some(body) => lower_block(module, builder, body),
            None => Ok(()),
        };
    }
    let then_index = builder.new_block(span);
    let else_index = builder.new_block(span);
    let after = builder.new_block(span);
    builder.finish(
        TerminatorKind::SwitchBool(condition_value, else_index, then_index),
        span,
    );

    let incoming = builder.owned.clone();
    let mut branch_ends: Vec<i64> = Vec::new();
    let mut branch_owners: Vec<Vec<i64>> = Vec::new();
    builder.select(then_index);
    lower_block(module, builder, then_body)?;
    if !builder.closed[builder.current] {
        branch_ends.push(builder.current as i64);
        branch_owners.push(builder.owned.clone());
    }

    builder.owned = incoming.clone();
    builder.select(else_index);
    if let Some(body) = else_body {
        lower_block(module, builder, body)?;
    }
    if !builder.closed[builder.current] {
        branch_ends.push(builder.current as i64);
        branch_owners.push(builder.owned.clone());
    }

    builder.owned = incoming;
    if !branch_ends.is_empty() {
        builder.owned = join_branch_owners(builder, after, &branch_ends, &branch_owners, span)?;
    }
    builder.select(after);
    builder.closed[after as usize] = branch_ends.is_empty();
    Ok(())
}

/// Read a literal or the latest whole-slot boolean literal in this block, never propagating across control-flow edges.
fn constant_boolean(builder: &Builder, operand: &Operand) -> Option<bool> {
    match &operand.kind {
        OperandKind::Literal(Constant::Bool(value)) => Some(*value),
        OperandKind::Copy(place) => {
            if place.projection != Projection::Whole {
                return None;
            }
            for statement in builder.blocks[builder.current].statements.iter().rev() {
                match &statement.kind {
                    StatementKind::Assign(target, value) => {
                        if target.local == place.local {
                            if target.projection != Projection::Whole {
                                return None;
                            }
                            return match &value.kind {
                                RvalueKind::Use(Operand {
                                    kind: OperandKind::Literal(Constant::Bool(flag)),
                                    ..
                                }) => Some(*flag),
                                _ => None,
                            };
                        }
                    }
                    StatementKind::StorageDead(local) => {
                        if *local == place.local {
                            return None;
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        _ => None,
    }
}

/// Lower a normalized loop: a header block that the body jumps back to, and an `after` block for `break`.
fn lower_loop(module: &BodyIrModule, builder: &mut Builder, body: &Block, span: &SourceSpan) -> Result<(), String> {
    let header = builder.new_block(span);
    let after = builder.new_block(span);
    builder.finish(TerminatorKind::Goto(header), span);
    builder.select(header);
    let incoming = builder.owned.clone();
    builder.loops.push((header, after));
    builder.loop_owners.push(incoming.clone());
    builder.loop_scopes.push(body.scope.0 as i64);
    builder.loop_exit_blocks.push(Vec::new());
    builder.loop_exit_owners.push(Vec::new());
    lower_block(module, builder, body)?;
    if !builder.closed[builder.current] {
        drop_loop_owners(builder, span)?;
    }
    builder.loops.pop();
    builder.loop_owners.pop();
    builder.loop_scopes.pop();
    let ends = builder.loop_exit_blocks.pop().unwrap_or_default();
    let owners = builder.loop_exit_owners.pop().unwrap_or_default();
    finish_fallthrough(builder, header, span);
    builder.owned = incoming;
    if !ends.is_empty() {
        builder.owned = join_branch_owners(builder, after, &ends, &owners, span)?;
    }
    builder.select(after);
    // Exhaustion and break share this exit; drop the iterator once, including any unvisited items.
    for statement in &body.stmts {
        if let SourceStatementKind::IterNext {
            iterator: SourceOperand::Place(read),
            ..
        } = &statement.kind
        {
            let receiver = scalar_place(builder, &read.place, span)?;
            if let PlanType::ListIntoIter(..) = place_type(&builder.locals, &receiver) {
                drop_owned(builder, receiver.local, span);
            }
        }
    }
    // A loop nothing leaves (`loop:` or `while true:` without `break`) never falls through to what follows it.
    builder.closed[after as usize] = !exits_to(&builder.blocks, header, after);
    Ok(())
}

/// Whether Body IR reads the payload's place under a shared or exclusive borrow.
pub fn borrowed_payload(payload: &SourceOperand) -> bool {
    match payload {
        SourceOperand::Place(read) => read.fact == OwnershipFact::Borrow || read.fact == OwnershipFact::MutBorrow,
        _ => false,
    }
}

/// In a body that carries a view, a reference payload Body IR borrows from a source place is that view's loan; any
/// other payload, such as an already borrowed parameter wrapped for a call, crosses the ordinary value boundary.
pub fn carrier_payload(
    builder: &mut Builder,
    payload: &SourceOperand,
    field: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if builder.carries_view
        && borrowed_payload(payload)
        && let Ok((_, writable)) = reference_contract(field)
    {
        let access = if writable {
            IncanInputAccess::Exclusive
        } else {
            IncanInputAccess::Shared
        };
        return view_operand(builder, payload, field, access, span);
    }
    boundary_operand(builder, payload, field, span)
}
