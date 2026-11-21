//! Enum matches and the owners live where their branches join.

use incan_mir_plan_native::{
    Operand, OperandKind, Place, PlanType, Projection, RvalueKind, SourceSpan, TerminatorKind, assign as plan_assign,
    place as plan_place,
};
use incan_semantics_core::body_ir::{
    BodyIrModule, MatchArm, Operand as SourceOperand, OwnershipFact, Pattern, PlaceElem,
};

use crate::carrier_operands::{bind_enum_fields, bind_payload_in_place, boundary_operand};
use crate::checked_values::{clone_nominal_operand, scalar_operand, scalar_place};
use crate::closures::callable_copy_type;
use crate::collections::{is_collection, is_owned_collection};
use crate::element_places::{element_place_type, is_element_place};
use crate::frame::{
    Builder, bind_read_value, dereferenced_place, drop_owned, finish_fallthrough, place_type, read_place, temporary,
    track_rvalue_moves,
};
use crate::hashed_collections::cloned_collection;
use crate::list_values::value_operand;
use crate::lists::is_list;
use crate::numerics::numeric_carrier;
use crate::options::{carrier_payload_value, clone_tuple_operand, variant_condition};
use crate::patterns::{
    binds_in_place, carrier_pattern_slot, enum_pattern_place, has_refutable_payload, nominal_field, pattern_binds,
    pattern_tests,
};
use crate::pending::untranslated;
use crate::statements::{is_nominal_type, lower_statement};
use crate::text_plan::clone_text;
use crate::tuples::{is_tuple_type, owned_carrier, tuple_fields, tuple_has_owners};

/// Close each `if` branch or match arm reaching the join, keeping live only the owners every such branch left live.
///
/// Body IR counts last uses across branches in source order, so one branch may move a value that another only borrows,
/// and a branch's own temporaries (a condition's constructed model, a comparison's enum value) end with it. Rust drops
/// such a value at scope end through a drop flag. Destroying text, collections, nominal values and lazy generators
/// only releases their storage, so releasing the value at the end of the branch that kept it is equivalent. An owner
/// of any other type left live on only some paths stays refused.
pub fn join_branch_owners(
    builder: &mut Builder,
    join: i64,
    ends: &[i64],
    arm_owners: &[Vec<i64>],
    span: &SourceSpan,
) -> Result<Vec<i64>, String> {
    let mut live: Vec<i64> = Vec::new();
    if let Some(first) = arm_owners.first()
        && !ends.is_empty()
    {
        for local in first {
            if arm_owners.iter().all(|owned| owned.contains(local)) {
                live.push(*local);
            }
        }
    }
    for (end, owners) in ends.iter().zip(arm_owners) {
        builder.select(*end);
        builder.owned = owners.clone();
        for local in owners {
            if live.contains(local) {
                continue;
            }
            if !releases_only_memory(&builder.locals[*local as usize].ty) {
                return Err("unsupported Body IR owned-value branch merge".into());
            }
            drop_owned(builder, *local, span);
        }
        finish_fallthrough(builder, join, span);
    }
    Ok(live)
}

/// Report whether destroying an owner only releases storage, including a lazy generator's retained state.
fn releases_only_memory(ty: &PlanType) -> bool {
    *ty == PlanType::String
        || is_owned_collection(ty)
        || matches!(
            ty,
            PlanType::Generator(..) | PlanType::Model(..) | PlanType::Enum(..) | PlanType::Tuple(_)
        )
}

/// Dispatch checked variants in source arm order, with independent owner snapshots on each branch.
pub fn lower_enum_match(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: Place,
    scrutinee: &SourceOperand,
    arms: &[MatchArm],
    span: &SourceSpan,
) -> Result<(), String> {
    if builder.owned.contains(&target.local) {
        return Err("unsupported Body IR initialized owned match destination".into());
    }
    let source = match_scrutinee_place(builder, scrutinee, arms, span)?;
    let PlanType::Enum(owner, _) = place_type(&builder.locals, &source) else {
        return lower_structural_match(module, builder, target, source, arms, span);
    };
    let join = builder.new_block(span);
    let mut owners = builder.owned.clone();
    let incoming = builder.owned.clone();
    let mark = builder.released.len();
    let mut arm_ends: Vec<i64> = Vec::new();
    let mut arm_owners: Vec<Vec<i64>> = Vec::new();
    for arm in arms {
        let selected = builder.new_block(span);
        let missed = builder.new_block(span);
        let next_arm = builder.new_block(span);
        let mut slot = -2;
        if binds_through_alternation(&arm.pattern) {
            let bound = bind_alternatives(builder, &source, &arm.pattern, selected, missed, &owners, span)?;
            builder.select(selected);
            builder.owned = bound;
        } else {
            slot = dispatch_enum_arm(builder, &source, owner, &arm.pattern, selected, missed, span)?;
            builder.select(selected);
            builder.owned = owners.clone();
            bind_enum_payload(builder, &source, slot, &arm.pattern, span)?;
        }
        let arm_target = ArmTarget {
            target: &target,
            owners: &owners,
            mark,
        };
        let consumed = lower_matched_arm(module, builder, arm, &arm_target, source.local, next_arm, span)?;
        if let Some(owned) = consumed.ended {
            arm_ends.push(builder.current as i64);
            arm_owners.push(owned);
        }
        owners = skip_consumed_guard(builder, missed, next_arm, &owners, &consumed.guard, span);
        // A catch-all arm ends the dispatch unless its guard can still reject.
        if slot == -1 && arm.guard.is_none() {
            break;
        }
    }
    finish_match(
        builder,
        &target,
        &incoming,
        mark,
        join,
        &arm_ends,
        &arm_owners,
        span,
        false,
    )
}

/// The destination and incoming ownership one arm writes against.
struct ArmTarget<'a> {
    target: &'a Place,
    owners: &'a [i64],
    mark: usize,
}

/// What one matched arm left: the owners its guard consumed and, when it reaches the join, its owners there.
struct ArmOutcome {
    guard: Vec<i64>,
    ended: Option<Vec<i64>>,
}

/// Run a matched arm's guard and body, then store its result and release its own bindings when it reaches the join.
fn lower_matched_arm(
    module: &BodyIrModule,
    builder: &mut Builder,
    arm: &MatchArm,
    destination: &ArmTarget<'_>,
    scrutinee: i64,
    next_arm: i64,
    span: &SourceSpan,
) -> Result<ArmOutcome, String> {
    let guard = lower_arm_guard(module, builder, arm, destination.owners, scrutinee, next_arm, span)?;
    lower_enum_arm_body(module, builder, arm)?;
    if builder.closed[builder.current] {
        return Ok(ArmOutcome { guard, ended: None });
    }
    let target = destination.target;
    let value = boundary_operand(builder, &arm.result, &place_type(&builder.locals, target), span)?;
    track_rvalue_moves(builder, &RvalueKind::Use(value.clone()));
    builder.emit(plan_assign(target.clone(), RvalueKind::Use(value), span));
    // Pattern-bound owners are branch locals; release them before the arm's owners meet the others at the join.
    release_arm_bindings(builder, destination.owners, destination.mark, span);
    Ok(ArmOutcome {
        guard,
        ended: Some(builder.owned.clone()),
    })
}

/// Close the dispatch, settle owners at the join and transfer an owned result to the destination.
#[allow(clippy::too_many_arguments)]
fn finish_match(
    builder: &mut Builder,
    target: &Place,
    incoming: &[i64],
    mark: usize,
    join: i64,
    arm_ends: &[i64],
    arm_owners: &[Vec<i64>],
    span: &SourceSpan,
    structural: bool,
) -> Result<(), String> {
    builder.finish(TerminatorKind::Unreachable, span);
    let (ends, kept) = release_unshared_replacements(builder, incoming, mark, arm_ends, arm_owners, span);
    // An arm or its guard may move an incoming owner the other arms keep; the join keeps only what every arm kept.
    let join_owners = join_branch_owners(builder, join, &ends, &kept, span)?;
    let reaches_join = !arm_ends.is_empty();
    builder.select(join);
    builder.closed[join as usize] = !reaches_join;
    builder.owned = join_owners;
    let ty = place_type(&builder.locals, target);
    let owned_result = ty == PlanType::String
        || is_nominal_type(&ty)
        || is_owned_collection(&ty)
        || (structural && tuple_has_owners(&ty));
    if reaches_join && owned_result {
        builder.owned.push(target.local);
    }
    Ok(())
}

/// Branch to `selected` when an arm's pattern matches the scrutinee and to `rejected` otherwise; return the variant.
///
/// A plain variant pattern tests its checked native variant. A pattern with a literal or nested payload, or an
/// alternation, runs the full pattern tests in source order. A wildcard selects every value (-1), and an alternation
/// names no single variant to bind (-2).
fn dispatch_enum_arm(
    builder: &mut Builder,
    source: &Place,
    owner: i64,
    pattern: &Pattern,
    selected: i64,
    rejected: i64,
    span: &SourceSpan,
) -> Result<i64, String> {
    if has_refutable_payload(pattern) {
        pattern_tests(builder, source, pattern, selected, rejected, span)?;
        if let Pattern::Or(_) = pattern {
            return Ok(-2);
        }
        return carrier_pattern_slot(builder, owner, pattern);
    }
    let slot = carrier_pattern_slot(builder, owner, pattern)?;
    if slot < 0 {
        builder.finish(TerminatorKind::Goto(selected), span);
        return Ok(slot);
    }
    let condition = variant_condition(builder, source, slot, span);
    builder.finish(TerminatorKind::SwitchBool(condition, rejected, selected), span);
    Ok(slot)
}

/// Report whether a pattern is an alternation whose alternatives bind names.
fn is_binding_alternation(pattern: &Pattern) -> bool {
    matches!(pattern, Pattern::Or(_)) && pattern_binds(pattern)
}

/// Report whether an arm's pattern binds through an alternation: at its top, or as an item of a tuple there.
fn binds_through_alternation(pattern: &Pattern) -> bool {
    if is_binding_alternation(pattern) {
        return true;
    }
    matches!(pattern, Pattern::Tuple(items) if items.iter().any(is_binding_alternation))
}

/// Test each alternative of a binding alternation in source order; the first that matches binds and selects the arm.
///
/// The checker requires every alternative to bind the same names, which share one local each, so every path reaches
/// `selected` owning the same bound locals. The owners at `selected` are returned. A pattern that is not itself an
/// alternation is its own single alternative.
fn bind_alternatives(
    builder: &mut Builder,
    source: &Place,
    pattern: &Pattern,
    selected: i64,
    rejected: i64,
    incoming: &[i64],
    span: &SourceSpan,
) -> Result<Vec<i64>, String> {
    let Pattern::Or(alternatives) = pattern else {
        return bind_alternative(builder, source, pattern, selected, rejected, incoming, span);
    };
    let mut bound: Vec<i64> = Vec::new();
    let mut first = true;
    for alternative in alternatives {
        let next_alternative = builder.new_block(span);
        let owners = bind_alternative(builder, source, alternative, selected, next_alternative, incoming, span)?;
        if first {
            bound = owners;
            first = false;
        } else if !same_owners(&owners, &bound) {
            return Err("unsupported Body IR binding alternation ownership".into());
        }
        builder.select(next_alternative);
    }
    builder.finish(TerminatorKind::Goto(rejected), span);
    Ok(bound)
}

/// Test one alternative and, when it matches, bind it and select the arm, returning the owners at `selected`.
fn bind_alternative(
    builder: &mut Builder,
    source: &Place,
    alternative: &Pattern,
    selected: i64,
    rejected: i64,
    incoming: &[i64],
    span: &SourceSpan,
) -> Result<Vec<i64>, String> {
    if let Pattern::Tuple(items) = alternative
        && binds_through_alternation(alternative)
    {
        let fields = tuple_fields(source, &place_type(&builder.locals, source), span)?;
        if fields.len() != items.len() {
            return Err("invalid Body IR tuple pattern layout".into());
        }
        let paths = TuplePaths {
            fields: &fields,
            items,
            selected,
            incoming,
        };
        return tuple_paths(builder, &paths, 0, vec![-1; items.len()], rejected, span);
    }
    let matched = builder.new_block(span);
    pattern_tests(builder, source, alternative, matched, rejected, span)?;
    builder.select(matched);
    builder.owned = incoming.to_vec();
    bind_structural_pattern(builder, source, alternative, span)?;
    let owners = builder.owned.clone();
    builder.finish(TerminatorKind::Goto(selected), span);
    Ok(owners)
}

/// One tuple pattern whose items bind through alternations, with the arm block every complete path selects.
struct TuplePaths<'a> {
    fields: &'a [Place],
    items: &'a [Pattern],
    selected: i64,
    incoming: &'a [i64],
}

/// Test a tuple pattern's items from `index` on, trying each alternative of an item that binds through an alternation.
///
/// `(A | B, C)` matches as `(A, C) | (B, C)`: a later item's failure tries the item's next alternative, and its last
/// alternative's failure rejects. A path on which every item matched binds each item, an alternation item through the
/// alternative `choices` records for it, and selects the arm. The owners at `selected` are returned.
fn tuple_paths(
    builder: &mut Builder,
    paths: &TuplePaths<'_>,
    index: usize,
    choices: Vec<i64>,
    rejected: i64,
    span: &SourceSpan,
) -> Result<Vec<i64>, String> {
    if index == paths.items.len() {
        builder.owned = paths.incoming.to_vec();
        for position in 0..paths.items.len() {
            bind_tuple_item(
                builder,
                &paths.fields[position],
                &paths.items[position],
                choices[position],
                span,
            )?;
        }
        let owners = builder.owned.clone();
        builder.finish(TerminatorKind::Goto(paths.selected), span);
        return Ok(owners);
    }
    if !is_binding_alternation(&paths.items[index]) {
        let next_item = builder.new_block(span);
        pattern_tests(
            builder,
            &paths.fields[index],
            &paths.items[index],
            next_item,
            rejected,
            span,
        )?;
        builder.select(next_item);
        return tuple_paths(builder, paths, index + 1, choices, rejected, span);
    }
    let Pattern::Or(alternatives) = &paths.items[index] else {
        return Err("invalid Body IR binding alternation".into());
    };
    let mut bound: Vec<i64> = Vec::new();
    for (choice, alternative) in alternatives.iter().enumerate() {
        let matched = builder.new_block(span);
        // A miss in this alternative or in any later item retries with the item's next alternative.
        let retry = builder.new_block(span);
        pattern_tests(builder, &paths.fields[index], alternative, matched, retry, span)?;
        builder.select(matched);
        let mut chosen = choices.clone();
        chosen[index] = choice as i64;
        let owners = tuple_paths(builder, paths, index + 1, chosen, retry, span)?;
        if choice == 0 {
            bound = owners;
        } else if !same_owners(&owners, &bound) {
            return Err("unsupported Body IR binding alternation ownership".into());
        }
        builder.select(retry);
    }
    builder.finish(TerminatorKind::Goto(rejected), span);
    Ok(bound)
}

/// Bind one tuple item: the alternative `choice` selects of an item that binds through an alternation, or the item.
fn bind_tuple_item(
    builder: &mut Builder,
    field: &Place,
    item: &Pattern,
    choice: i64,
    span: &SourceSpan,
) -> Result<(), String> {
    if choice < 0 {
        return bind_structural_pattern(builder, field, item, span);
    }
    match item {
        Pattern::Or(alternatives) => bind_structural_pattern(builder, field, &alternatives[choice as usize], span),
        _ => Err("invalid Body IR binding alternation".into()),
    }
}

/// Report whether two owner ledgers hold the same locals, in any order.
fn same_owners(left: &[i64], right: &[i64]) -> bool {
    left.len() == right.len() && left.iter().all(|local| right.contains(local))
}

/// Select the place a match inspects, borrowing the matched storage itself when an arm changes it in place.
///
/// A list element is read as a copy unless an arm changes its payload in place; then it is reborrowed in its list, as
/// legacy's checked `list_get_mut` does. A local, a field of one, a `mut` parameter and an exclusive loop item are
/// already places the change reaches. A copied loop variable is not, so an in-place match over one stays refused.
fn match_scrutinee_place(
    builder: &mut Builder,
    scrutinee: &SourceOperand,
    arms: &[MatchArm],
    span: &SourceSpan,
) -> Result<Place, String> {
    let in_place = arms.iter().any(|arm| binds_in_place(&arm.pattern));
    let SourceOperand::Place(read) = scrutinee else {
        return Err("unsupported Body IR nonplace enum match scrutinee".into());
    };
    let indexed = read
        .place
        .projection
        .iter()
        .any(|projection| matches!(projection, PlaceElem::Index(_)));
    if indexed && in_place {
        return Err(untranslated("matches::exclusive_element"));
    }
    if indexed {
        // Collection reads clone one element before inspecting its tag. Evaluate the checked index once and keep the
        // parent alive; an index cannot be interpreted as a model field projection.
        let operand = value_operand(builder, scrutinee, span)?;
        return match operand.kind {
            OperandKind::Move(place) | OperandKind::Copy(place) => Ok(place),
            _ => Err("unsupported Body IR nonplace enum match scrutinee".into()),
        };
    }
    let source = scalar_place(builder, &read.place, span)?;
    if in_place && builder.borrowed_items.contains(&source.local) {
        return Err("unsupported Body IR in-place match on a copied loop variable".into());
    }
    Ok(enum_pattern_place(builder, source, span))
}

/// Admit only an owned list referent with checked exclusive variable bindings or ignored wildcard arms.
fn validate_in_place_list_match(source: &PlanType, arms: &[MatchArm]) -> Result<bool, String> {
    if !matches!(source, PlanType::List(..)) {
        return Ok(false);
    }
    for arm in arms {
        match &arm.pattern {
            Pattern::Var(binding) => {
                if binding.fact != OwnershipFact::MutBorrow {
                    return Err("unsupported Body IR list match without an in-place binding".into());
                }
            }
            Pattern::Wildcard => {}
            _ => return Err("unsupported Body IR list match pattern".into()),
        }
    }
    Ok(true)
}

/// Dispatch primitive and structural patterns, retaining checked list bindings as references to matched storage.
fn lower_structural_match(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: Place,
    source: Place,
    arms: &[MatchArm],
    span: &SourceSpan,
) -> Result<(), String> {
    let source_type = place_type(&builder.locals, &source);
    // An inferred-shared text input lends its text; the patterns test that text through the reference.
    if source_type == PlanType::StringRef && source.projection == Projection::Whole {
        let lent = Place {
            local: source.local,
            projection: Projection::Deref(PlanType::String),
            span: source.span,
        };
        return lower_structural_match(module, builder, target, lent, arms, span);
    }
    let in_place_list = validate_in_place_list_match(&source_type, arms)?;
    if !numeric_carrier(&source_type)
        && !matches!(
            source_type,
            PlanType::Bool | PlanType::Unit | PlanType::String | PlanType::Model(..)
        )
        && !is_tuple_type(&source_type)
        && !in_place_list
    {
        return Err("unsupported Body IR nonprimitive structural match scrutinee".into());
    }
    let join = builder.new_block(span);
    let mut owners = builder.owned.clone();
    let incoming = builder.owned.clone();
    let mark = builder.released.len();
    let mut arm_ends: Vec<i64> = Vec::new();
    let mut arm_owners: Vec<Vec<i64>> = Vec::new();
    for arm in arms {
        let selected = builder.new_block(span);
        let missed = builder.new_block(span);
        let next_arm = builder.new_block(span);
        if binds_through_alternation(&arm.pattern) {
            let bound = bind_alternatives(builder, &source, &arm.pattern, selected, missed, &owners, span)?;
            builder.select(selected);
            builder.owned = bound;
        } else {
            pattern_tests(builder, &source, &arm.pattern, selected, missed, span)?;
            builder.select(selected);
            bind_structural_pattern(builder, &source, &arm.pattern, span)?;
        }
        let arm_target = ArmTarget {
            target: &target,
            owners: &owners,
            mark,
        };
        let consumed = lower_matched_arm(module, builder, arm, &arm_target, source.local, next_arm, span)?;
        if let Some(owned) = consumed.ended {
            arm_ends.push(builder.current as i64);
            arm_owners.push(owned);
        }
        owners = skip_consumed_guard(builder, missed, next_arm, &owners, &consumed.guard, span);
    }
    finish_match(
        builder,
        &target,
        &incoming,
        mark,
        join,
        &arm_ends,
        &arm_owners,
        span,
        true,
    )
}

/// Run a matched arm's guard after its pattern bound, falling through to `rejected` when the guard is false.
///
/// The guard reads the arm's bindings and the incoming owners. A rejecting guard releases the bindings and guard
/// temporaries it owns before falling through; an admitting guard keeps them for the arm's body. The guard may make the
/// last use of an incoming owner, moving it on every path through the guard (Body IR counts last uses in source order,
/// so no later arm reads it); those owners are returned for the caller to release on the paths that skip the guard. The
/// scrutinee, which later arms test, and an owner whose release does more than free memory stay refused.
fn lower_arm_guard(
    module: &BodyIrModule,
    builder: &mut Builder,
    arm: &MatchArm,
    incoming: &[i64],
    scrutinee: i64,
    rejected: i64,
    span: &SourceSpan,
) -> Result<Vec<i64>, String> {
    let mut consumed = Vec::new();
    let Some(guard) = &arm.guard else {
        return Ok(consumed);
    };
    for statement in &arm.guard_stmts {
        lower_statement(module, builder, statement)?;
        if builder.closed[builder.current] {
            return Err("unsupported Body IR diverging match guard".into());
        }
    }
    let condition = scalar_operand(builder, guard, span)?;
    for local in incoming {
        if !builder.owned.contains(local) {
            if *local == scrutinee || !releases_only_memory(&builder.locals[*local as usize].ty) {
                return Err("unsupported Body IR match guard consuming an owner".into());
            }
            consumed.push(*local);
        }
    }
    let admitted = builder.new_block(span);
    let refused = builder.new_block(span);
    builder.finish(TerminatorKind::SwitchBool(condition, refused, admitted), span);
    let kept = builder.owned.clone();
    builder.select(refused);
    release_match_scratch(builder, incoming, span);
    builder.finish(TerminatorKind::Goto(rejected), span);
    builder.select(admitted);
    builder.owned = kept;
    Ok(consumed)
}

/// Close an arm's pattern-miss path into the next arm and return the owners the next arm starts with.
///
/// The arm's guard moved `consumed` on every path through it, so the miss path, which skips the guard, releases them
/// too; both ways into the next arm then hold the same owners.
fn skip_consumed_guard(
    builder: &mut Builder,
    missed: i64,
    next_arm: i64,
    owners: &[i64],
    consumed: &[i64],
    span: &SourceSpan,
) -> Vec<i64> {
    builder.select(missed);
    builder.owned = owners.to_vec();
    for local in consumed {
        drop_owned(builder, *local, span);
    }
    builder.finish(TerminatorKind::Goto(next_arm), span);
    let remaining: Vec<i64> = owners
        .iter()
        .copied()
        .filter(|local| !consumed.contains(local))
        .collect();
    builder.select(next_arm);
    builder.owned = remaining.clone();
    remaining
}

/// Release branch-local bindings and literal materializations without dropping incoming owners.
fn release_match_scratch(builder: &mut Builder, incoming: &[i64], span: &SourceSpan) {
    let owners = builder.owned.clone();
    for local in owners {
        if !incoming.contains(&local) {
            drop_owned(builder, local, span);
        }
    }
}

/// Release what an arm bound or built for itself when the arm ends, keeping a value it gave an enclosing local.
///
/// An owner the match did not start with is the arm's own, unless it moved or dropped before the match: that one was
/// declared outside, held no value when the match started, and the arm gave it a new one that later statements read.
fn release_arm_bindings(builder: &mut Builder, owners: &[i64], mark: usize, span: &SourceSpan) {
    let held = builder.owned.clone();
    for local in held {
        if !owners.contains(&local) && !released_before(builder, local, mark) {
            drop_owned(builder, local, span);
        }
    }
}

/// Report whether `local` moved or dropped among the first `mark` releases of this frame.
fn released_before(builder: &Builder, local: i64, mark: usize) -> bool {
    builder.released[..mark.min(builder.released.len())].contains(&local)
}

/// Drop a value an arm gave an enclosing local that held none when the match started, unless every arm reaching the
/// join gave it one: only then does the local hold a value on every path after the match.
#[allow(clippy::type_complexity)]
fn release_unshared_replacements(
    builder: &mut Builder,
    incoming: &[i64],
    mark: usize,
    ends: &[i64],
    arm_owners: &[Vec<i64>],
    span: &SourceSpan,
) -> (Vec<i64>, Vec<Vec<i64>>) {
    let mut settled_ends = Vec::with_capacity(ends.len());
    let mut settled_owners = Vec::with_capacity(ends.len());
    for (end, owners) in ends.iter().zip(arm_owners) {
        builder.select(*end);
        builder.owned = owners.clone();
        for local in owners {
            if incoming.contains(local) || arm_owners.iter().all(|kept| kept.contains(local)) {
                continue;
            }
            if released_before(builder, *local, mark) {
                drop_owned(builder, *local, span);
            }
        }
        settled_ends.push(builder.current as i64);
        settled_owners.push(builder.owned.clone());
    }
    (settled_ends, settled_owners)
}

/// Initialize structural bindings through the retained payload ownership policy after every component matched.
pub fn bind_structural_pattern(
    builder: &mut Builder,
    source: &Place,
    pattern: &Pattern,
    span: &SourceSpan,
) -> Result<(), String> {
    match pattern {
        Pattern::Var(binding) => {
            let local = binding.local.index() as i64 + 1 + builder.local_offset;
            let ty = place_type(&builder.locals, source);
            if builder.locals[local as usize].ty != ty {
                // A borrowed subject binds its components as shared references to them.
                let reference = borrowed_binding_type(&ty);
                if reference == PlanType::Unit || builder.locals[local as usize].ty != reference {
                    return Err("invalid Body IR structural pattern binding carrier".into());
                }
                builder.emit(plan_assign(
                    plan_place(local, span),
                    RvalueKind::Borrow(source.clone()),
                    span,
                ));
                return Ok(());
            }
            if binding.fact == OwnershipFact::MutBorrow && is_list(&ty) {
                return bind_payload_in_place(builder, local, source.clone(), span);
            }
            match binding.fact {
                OwnershipFact::Unknown => return Err("unsupported Body IR pattern binding ownership Unknown".into()),
                OwnershipFact::MutBorrow => return Err("unsupported Body IR mutable pattern binding".into()),
                _ => {}
            }
            // A component with owned storage is read through the payload policy, which clones or moves it; any other is
            // copied. Either way the binding owns exactly what its read produced.
            let value =
                if ty == PlanType::String || tuple_has_owners(&ty) || owned_carrier(&ty) || is_owned_collection(&ty) {
                    enum_payload_read(builder, source.clone(), binding.fact, span)?
                } else {
                    read_place(source.clone(), false, span)
                };
            bind_read_value(builder, local, value, span);
        }
        Pattern::Tuple(items) => {
            let ty = place_type(&builder.locals, source);
            let fields = tuple_fields(source, &ty, span)?;
            if fields.len() != items.len() {
                return Err("invalid Body IR tuple pattern binding layout".into());
            }
            for (field, item) in fields.iter().zip(items) {
                bind_structural_pattern(builder, field, item, span)?;
            }
        }
        Pattern::Nominal { target, fields } => {
            for (name, field_pattern) in fields {
                let field = nominal_field(builder, source, target, name, span)?;
                // A field binding reads out of the borrowed scrutinee, so it may only copy the value or clone text.
                if let Pattern::Var(_) = field_pattern
                    && !structural_binding_carrier(&place_type(&builder.locals, &field))
                {
                    return Err("unsupported Body IR owned model field pattern binding".into());
                }
                bind_structural_pattern(builder, &field, field_pattern, span)?;
            }
        }
        Pattern::Or(_) => {
            // A carrier pattern inside a structure binds its payload names like a match over that carrier.
            if pattern_binds(pattern) {
                return Err("unsupported Body IR binding alternation".into());
            }
        }
        Pattern::Wildcard | Pattern::Literal(_) => {}
        _ => {
            if pattern_binds(pattern) {
                let carrier = enum_pattern_place(builder, source.clone(), span);
                let PlanType::Enum(owner, _) = place_type(&builder.locals, &carrier) else {
                    return Err("unsupported Body IR structural pattern binding".into());
                };
                let slot = carrier_pattern_slot(builder, owner, pattern)?;
                return bind_enum_payload(builder, &carrier, slot, pattern, span);
            }
        }
    }
    Ok(())
}

/// The shared-reference carrier a binding takes to a matched scalar, text or model component; Unit when none.
fn borrowed_binding_type(ty: &PlanType) -> PlanType {
    if numeric_carrier(ty) || *ty == PlanType::Bool {
        return PlanType::ScalarRef(vec![ty.clone()]);
    }
    match ty {
        PlanType::String => PlanType::StringRef,
        PlanType::Model(index, name) => PlanType::ModelRef(*index, name.clone()),
        _ => PlanType::Unit,
    }
}

/// Report whether a binding copies its matched value or clones it as text.
fn structural_binding_carrier(ty: &PlanType) -> bool {
    numeric_carrier(ty) || matches!(ty, PlanType::Bool | PlanType::Unit | PlanType::String)
}

/// Borrow the retained arm's statements and stop once its control flow terminates.
fn lower_enum_arm_body(module: &BodyIrModule, builder: &mut Builder, arm: &MatchArm) -> Result<(), String> {
    for statement in &arm.body_stmts {
        lower_statement(module, builder, statement)?;
        if builder.closed[builder.current] {
            return Ok(());
        }
    }
    Ok(())
}

/// Bind payloads through native downcasts, preserving checked copies, borrows, clones and single-owner moves.
pub fn bind_enum_payload(
    builder: &mut Builder,
    source: &Place,
    variant: i64,
    pattern: &Pattern,
    span: &SourceSpan,
) -> Result<(), String> {
    match pattern {
        Pattern::UnionMember { fields, .. } | Pattern::Enum { fields, .. } | Pattern::Result { fields, .. } => {
            bind_enum_fields(builder, source, variant, fields, span)
        }
        _ => Ok(()),
    }
}

/// Preserve checked native reference payloads and copy facts; clone only payloads with owned storage.
pub fn enum_payload_read(
    builder: &mut Builder,
    place: Place,
    fact: OwnershipFact,
    span: &SourceSpan,
) -> Result<Operand, String> {
    // Legacy matches a cloned enum by value, so a moved payload binding is materialized like a cloned one.
    match fact {
        OwnershipFact::Copy => return Ok(read_place(place, false, span)),
        OwnershipFact::Clone | OwnershipFact::Borrow | OwnershipFact::Move => {}
        _ => return Err("unsupported Body IR enum payload binding ownership".into()),
    }
    let payload_type = place_type(&builder.locals, &place);
    // The canonical frozen wrapper's Clone is its Copy value; it owns no allocation or custom clone hook. A type
    // parameter's payload binding is cloned by its checked fact, which is Rust's copy-valued Clone once the instance
    // makes the payload a concrete scalar.
    if payload_type == PlanType::FrozenStr || callable_copy_type(&payload_type) {
        return Ok(read_place(place, false, span));
    }
    match &payload_type {
        PlanType::ModelRef(..)
        | PlanType::EnumRef(..)
        | PlanType::SliceRef(_)
        | PlanType::NativeArrayRef(..)
        | PlanType::StrRef => return Ok(read_place(place, false, span)),
        PlanType::ModelMutRef(..) | PlanType::EnumMutRef(..) | PlanType::StringMutRef | PlanType::TupleMutRef(_) => {
            return reborrowed_payload(builder, place, payload_type, span);
        }
        _ => {}
    }
    if numeric_carrier(&payload_type) || matches!(payload_type, PlanType::Bool | PlanType::Unit) {
        return Ok(read_place(place, false, span));
    }
    if is_tuple_type(&payload_type) {
        return clone_tuple_operand(builder, place, span);
    }
    if is_list(&payload_type) {
        return carrier_payload_value(builder, place, span);
    }
    if is_collection(&payload_type) {
        return cloned_collection(builder, place, span);
    }
    // Legacy matches a cloned enum by value. Cloning just the selected payload preserves that owned-binding contract
    // while the canonical match scrutinee stays borrowed; unselected payloads remain owned by their original enum.
    if payload_type == PlanType::String {
        let cloned = clone_text(builder, place, span)?;
        return Ok(read_place(cloned, true, span));
    }
    clone_nominal_operand(builder, place, span)
}

/// Lend an exclusive reference payload's referent again, as `Option.as_mut()` hands it out. An exclusive reference can
/// be neither copied nor cloned; reborrowing leaves the carrier whole for any later reader.
fn reborrowed_payload(
    builder: &mut Builder,
    place: Place,
    reference: PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let referent = match &reference {
        PlanType::StringMutRef => PlanType::String,
        PlanType::ModelMutRef(owner, name) => PlanType::Model(*owner, name.clone()),
        PlanType::EnumMutRef(owner, name) => PlanType::Enum(*owner, name.clone()),
        PlanType::TupleMutRef(elements) => PlanType::Tuple(elements.clone()),
        _ => return Err("invalid Body IR mutable parameter reference".into()),
    };
    let referent = dereferenced_place(&place, referent, span)?;
    let reborrow = temporary(builder, reference, span);
    builder.emit(plan_assign(reborrow.clone(), RvalueKind::MutBorrow(referent), span));
    Ok(read_place(reborrow, true, span))
}

/// Classify the retained value type without reading an index; enum comparison materializes the value only once.
pub fn enum_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<bool, String> {
    let SourceOperand::Place(read) = value else {
        return Ok(false);
    };
    // Retained static cells are scalar-only; the ordinary read validates their canonical storage identity.
    if read.place.global().is_some() {
        return Ok(false);
    }
    let ty = if is_element_place(builder, &read.place)? {
        element_place_type(builder, &read.place)?
    } else {
        let place = scalar_place(builder, &read.place, span)?;
        place_type(&builder.locals, &place)
    };
    Ok(matches!(ty, PlanType::Enum(..)))
}
