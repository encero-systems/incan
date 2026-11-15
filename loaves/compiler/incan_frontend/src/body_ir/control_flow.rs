//! Lowering for conditional and looping control flow, and the iteration protocol behind `for`.

use super::args::*;
use super::primitives::*;
use super::reads::*;
use super::refusals::*;
use super::*;

mod branches_and_loops;
mod enumerated_and_zipped;
mod iteration;
mod range_loops;

/// Where one range-shaped `for` loop takes its bounds, step, and inclusivity from.
///
/// Inline headers, materialized aggregates, and checker-proven builtin calls retain their own layout authority while
/// driving the same normalized counting loop. Each evaluates its range once, before the first iteration, as legacy's
/// native range does.
enum RangeLoopSource {
    /// An inline `start..end` / `start..=end` loop header: its start, evaluated first, and the local its end was
    /// snapshotted into.
    Header {
        start: bir::Operand,
        end: bir::LocalId,
        inclusive: bool,
    },
    /// A snapshot of a materialized [`bir::AggregateKind::Range`] value, read back through its own declared fields.
    Value(bir::Place),
    /// Checked builtin range arguments, snapshotted in source order before the first iteration.
    Builtin(Box<BuiltinRangeBounds>),
}

/// Scalar operands retained once from a checked builtin call; boxed together to keep range classification compact.
struct BuiltinRangeBounds {
    start: bir::Operand,
    end: bir::Operand,
    step: bir::Operand,
}

/// The independently retained storage or scalar state of one canonical direct zip operand.
enum ZipLoopSource {
    /// An original list and the initializer whose checked borrow mode its component's mutation settles.
    List { iterator: bir::LocalId, initializer: usize },
    /// Once-evaluated builtin bounds and the next scalar item, without materializing a range as a list.
    Range {
        bounds: Box<BuiltinRangeBounds>,
        index: bir::LocalId,
    },
}

/// Read a counting loop's index local, or the local a header's end bound was snapshotted into.
///
/// Both are compiler-owned scalar temporaries the loop re-reads several times per iteration, so their reads are always
/// plain [`bir::OwnershipFact::Copy`] and never a last use. Going through [`BodyBuilder::ownership_fact_for_place`]
/// would consult a read countdown that was never seeded for a temporary.
fn index_read(idx_local: bir::LocalId) -> bir::Operand {
    bir::Operand::place(bir::Place::from_local(idx_local), bir::OwnershipFact::Copy, false)
}

/// Retain a local fact only when every supplied continuing control-flow state retains it.
pub(super) fn intersect_local_states(
    states: Vec<std::collections::HashSet<bir::LocalId>>,
) -> std::collections::HashSet<bir::LocalId> {
    let mut states = states.into_iter();
    let Some(mut common) = states.next() else {
        return std::collections::HashSet::new();
    };
    for state in states {
        common.retain(|local| state.contains(local));
    }
    common
}

/// Return the immediate inline range parts through redundant source parentheses.
fn inline_range_parts(
    expr: &ast::Spanned<ast::Expr>,
) -> Option<(&ast::Spanned<ast::Expr>, &ast::Spanned<ast::Expr>, bool)> {
    match &expr.node {
        ast::Expr::Range { start, end, inclusive } => Some((start, end, *inclusive)),
        ast::Expr::Paren(inner) => inline_range_parts(inner),
        _ => None,
    }
}

/// Recognize only zero-based, unit-step builtin argument layouts before any argument is evaluated.
/// Canonical builtin identity is checked separately; computed starts and steps retain ordinary iteration.
fn zero_based_unit_arguments(arguments: &[ast::CallArg]) -> bool {
    let integer = |argument: &ast::CallArg, expected| {
        matches!(argument, ast::CallArg::Positional(value)
            if matches!(&value.node, ast::Expr::Literal(ast::Literal::Int(number))
                if number.value == expected && number.suffix.is_none()))
    };
    match arguments {
        [ast::CallArg::Positional(end)] => inline_range_parts(end).is_none(),
        [start, ast::CallArg::Positional(_)] => integer(start, 0),
        [start, ast::CallArg::Positional(_), step] => integer(start, 0) && integer(step, 1),
        _ => false,
    }
}

/// Report whether a pattern binds any name exclusively, which marks a pattern matched in place.
fn pattern_binds_in_place(pattern: &bir::Pattern) -> bool {
    match pattern {
        bir::Pattern::Var(binding) => binding.fact == bir::OwnershipFact::MutBorrow,
        bir::Pattern::Tuple(items) | bir::Pattern::Or(items) => items.iter().any(pattern_binds_in_place),
        bir::Pattern::Enum { fields, .. }
        | bir::Pattern::Result { fields, .. }
        | bir::Pattern::UnionMember { fields, .. } => fields.iter().any(pattern_binds_in_place),
        bir::Pattern::Struct { fields, .. } | bir::Pattern::Nominal { fields, .. } => {
            fields.iter().any(|(_, field)| pattern_binds_in_place(field))
        }
        _ => false,
    }
}

/// Report whether an operand borrows `item`, or a part of it, exclusively.
fn borrows_item_exclusively(operand: &bir::Operand, item: bir::LocalId) -> bool {
    matches!(
        operand,
        bir::Operand::Place(read)
            if read.fact == bir::OwnershipFact::MutBorrow && read.place.local_id() == Some(item)
    )
}

/// Read a builtin loop's iterable exclusively, in the statement that materializes the loop's iterator from it.
fn borrow_iterable_exclusively(statement: &mut bir::Statement) {
    if let bir::StatementKind::Assign {
        rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
        ..
    } = &mut statement.kind
    {
        read.fact = bir::OwnershipFact::MutBorrow;
    }
}
