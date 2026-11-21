//! Loop exits: whether a loop's blocks ever reach the block after it.

use incan_mir_plan_native::{BasicBlock, TerminatorKind};

/// Report whether any block from `start` onward has an edge to `target`.
///
/// A loop's exit block is allocated before its body and only the body's own blocks can leave the loop, so a loop whose
/// blocks never reach the exit never falls through: the exit stays unreachable, as Rust's `loop` without `break` is.
pub fn exits_to(blocks: &[BasicBlock], start: i64, target: i64) -> bool {
    blocks
        .iter()
        .skip(start.max(0) as usize)
        .any(|block| edges_to(&block.terminator.kind, target))
}

/// Whether one terminator names `target` as a normal successor; cleanup edges never leave a loop normally.
fn edges_to(kind: &TerminatorKind, target: i64) -> bool {
    match kind {
        TerminatorKind::Goto(next) => *next == target,
        TerminatorKind::SwitchBool(_, otherwise, then) => *otherwise == target || *then == target,
        TerminatorKind::Call(_, _, _, next, _) => *next == target,
        TerminatorKind::Drop(_, next, _) => *next == target,
        TerminatorKind::AssertOverflow(_, _, _, _, next, _) => *next == target,
        _ => false,
    }
}
