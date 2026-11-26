//! Lazy generators of pairs: pair adapters, builtin `zip` over iterators, and loop iterators reading them.

use incan_lang::lang::surface::iterator_methods::IteratorMethodId;
use incan_mir_plan_native::{ListLeaf, PlanType};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{
    ArgumentElement, Block, Body, BuiltinFnId, CallableTarget, Callee, MatchArm, MethodTarget, Operand, Place, Rvalue,
    StatementKind,
};

/// Name the frame slots, one past each Body IR local, that hold a lazy generator of pairs.
///
/// The checker types builtin `zip` over lists and a pair adapter alike as `Iterator[tuple[A, B]]`. Builtin `zip` over
/// lists keeps its list-snapshot layout. A pair adapter (`enumerate` or `zip` called on an iterator), builtin `zip`
/// over two iterators, and a loop iterator reading either of them draw lazily from their sources instead.
pub fn pair_generator_locals(body: &Body) -> Vec<i64> {
    let mut pairs = Vec::new();
    collect_pairs(body, &body.block, &mut pairs);
    pairs
}

/// Lay out a pair iterator slot as the generator of those pairs; any other layout keeps its own refusal.
pub fn pair_generator_type(ty: PlanType) -> PlanType {
    match ty {
        PlanType::ZipIterator(left, right) => PlanType::Generator(ListLeaf::Tuple(vec![left, right]), 0),
        _ => ty,
    }
}

/// Visit the statements in order, so a loop iterator follows the pair slot its source call filled before it.
fn collect_pairs(body: &Body, block: &Block, pairs: &mut Vec<i64>) {
    for statement in &block.stmts {
        visit_statement(body, &statement.kind, pairs);
    }
}

/// Record a pair-producing call's destination or a loop iterator reading a pair slot, and visit nested blocks.
fn visit_statement(body: &Body, kind: &StatementKind, pairs: &mut Vec<i64>) {
    match kind {
        StatementKind::Call {
            destination,
            callee,
            args,
            ..
        } => {
            if produces_pairs(body, callee, args)
                && let Some(place) = destination
            {
                add_slot(place, pairs);
            }
        }
        StatementKind::Assign { place, rvalue } => {
            if reads_pair_slot(rvalue, pairs) {
                add_slot(place, pairs);
            }
            if let Rvalue::Match { arms, .. } = rvalue {
                for arm in arms {
                    collect_arm_pairs(body, arm, pairs);
                }
            }
        }
        StatementKind::UnsafeRegion { body: nested, .. } => collect_pairs(body, nested, pairs),
        StatementKind::Race { arms, .. } => {
            for arm in arms {
                collect_pairs(body, &arm.body, pairs);
            }
        }
        StatementKind::Loop { body: nested } => collect_pairs(body, nested, pairs),
        StatementKind::If {
            then_block, else_block, ..
        } => {
            collect_pairs(body, then_block, pairs);
            if let Some(nested) = else_block {
                collect_pairs(body, nested, pairs);
            }
        }
        _ => {}
    }
}

/// Recognize a pair adapter by its builtin identity, or builtin `zip` by its two iterator operands.
fn produces_pairs(body: &Body, callee: &Callee, arguments: &[ArgumentElement]) -> bool {
    match callee {
        Callee::Method(target) => is_pair_adapter(target),
        Callee::Function(CallableTarget::Named(target)) => {
            target.builtin == Some(BuiltinFnId::Zip) && zips_iterators(body, arguments)
        }
        _ => false,
    }
}

/// Match checker-selected `enumerate` and `zip` operations on generators and iterator values.
fn is_pair_adapter(target: &MethodTarget) -> bool {
    if let Some(method) = target.builtin_iterator() {
        return matches!(method, IteratorMethodId::Enumerate | IteratorMethodId::Zip);
    }
    matches!(
        target.builtin_generator(),
        Some(IteratorMethodId::Enumerate | IteratorMethodId::Zip)
    )
}

/// Require exactly two whole-local operands whose checked types are iterators or generators.
fn zips_iterators(body: &Body, arguments: &[ArgumentElement]) -> bool {
    if arguments.len() != 2 {
        return false;
    }
    arguments.iter().all(|argument| match argument.as_one() {
        Some(Operand::Place(read)) => iterator_local(body, &read.place),
        _ => false,
    })
}

/// Read a whole local's checked type; a projection or a global is not an iterator slot.
fn iterator_local(body: &Body, place: &Place) -> bool {
    if !place.projection.is_empty() {
        return false;
    }
    match place.local_id() {
        Some(local) => is_iterator_type(&body.locals[local.index()].ty),
        None => false,
    }
}

/// Recognize a checked `Iterator[T]` or `Generator[T]` type.
fn is_iterator_type(ty: &IncanType) -> bool {
    matches!(ty, IncanType::Generic { base, .. } if base == "Iterator" || base == "Generator")
}

/// Recognize a loop iterator initialized from a whole pair slot.
fn reads_pair_slot(value: &Rvalue, pairs: &[i64]) -> bool {
    match value {
        Rvalue::Use(Operand::Place(read)) => {
            if !read.place.projection.is_empty() {
                return false;
            }
            match read.place.local_id() {
                Some(local) => pairs.contains(&(local.index() as i64 + 1)),
                None => false,
            }
        }
        _ => false,
    }
}

/// Record a whole local's frame slot once.
fn add_slot(place: &Place, pairs: &mut Vec<i64>) {
    if !place.projection.is_empty() {
        return;
    }
    if let Some(local) = place.local_id() {
        let slot = local.index() as i64 + 1;
        if !pairs.contains(&slot) {
            pairs.push(slot);
        }
    }
}

/// Borrow same-frame match statements without moving them out of the checked Body IR.
fn collect_arm_pairs(body: &Body, arm: &MatchArm, pairs: &mut Vec<i64>) {
    for statement in arm.guard_stmts.iter().chain(&arm.body_stmts) {
        visit_statement(body, &statement.kind, pairs);
    }
}
