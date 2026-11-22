//! List layout and iteration admission from canonical checked Body IR.

use incan_mir_plan_native::{ListLeaf, PlanType};
use incan_semantics_core::body_ir::{Block, IterProtocol, MatchArm, Operand, OwnershipFact, Rvalue, StatementKind};

use crate::collections::{collection_reference, leaf_type};

/// An owned or borrowed list's admitted leaf and nesting depth.
pub fn list_shape(ty: &PlanType) -> Option<(ListLeaf, i64)> {
    match ty {
        PlanType::List(leaf, depth) | PlanType::ListRef(leaf, depth) | PlanType::ListMutRef(leaf, depth) => {
            Some((leaf.clone(), *depth))
        }
        _ => None,
    }
}

/// A proven list layout or the caller's construct-specific refusal.
pub fn require_list_shape(ty: &PlanType, refusal: &str) -> Result<(ListLeaf, i64), String> {
    list_shape(ty).ok_or_else(|| refusal.to_owned())
}

/// The exact immediate element representation without flattening nested lists.
pub fn list_element(leaf: &ListLeaf, depth: i64) -> PlanType {
    if depth > 1 {
        return PlanType::List(leaf.clone(), depth - 1);
    }
    leaf_type(leaf)
}

/// All builtin cursor storage; the checked initializer separately chooses borrowed or consuming storage.
pub fn list_iterator_locals(block: &Block) -> Vec<i64> {
    let mut result = Vec::new();
    for statement in &block.stmts {
        result.extend(statement_iterator_locals(&statement.kind));
    }
    result
}

/// Inspect the typed canonical statement enum, including nested checked control-flow blocks.
pub fn statement_iterator_locals(kind: &StatementKind) -> Vec<i64> {
    let mut result = Vec::new();
    match kind {
        StatementKind::IterNext { iterator, protocol, .. } => {
            if is_builtin_iteration(protocol)
                && let Operand::Place(read) = iterator
                && let Some(local) = read.place.local_id()
            {
                result.push(local.index() as i64 + 1);
            }
        }
        StatementKind::Assign {
            rvalue: Rvalue::Match { arms, .. },
            ..
        } => {
            for arm in arms {
                collect_arm_iterator_locals(arm, &mut result);
            }
        }
        StatementKind::UnsafeRegion { body, .. } => result.extend(list_iterator_locals(body)),
        StatementKind::Race { arms, .. } => {
            for arm in arms {
                result.extend(list_iterator_locals(&arm.body));
            }
        }
        StatementKind::Loop { body } => result.extend(list_iterator_locals(body)),
        StatementKind::If {
            then_block, else_block, ..
        } => {
            result.extend(list_iterator_locals(then_block));
            if let Some(body) = else_block {
                result.extend(list_iterator_locals(body));
            }
        }
        _ => {}
    }
    result
}

pub fn is_list(ty: &PlanType) -> bool {
    list_shape(ty).is_some()
}

/// Only vectors own contents; references must never enter the drop ledger.
pub fn is_owned_list(ty: &PlanType) -> bool {
    matches!(ty, PlanType::List(..))
}

pub fn is_builtin_iteration(protocol: &IterProtocol) -> bool {
    matches!(protocol, IterProtocol::Builtin)
}

/// Map supported collection parameters and iterable storage to caller-owned references.
pub fn list_parameter_type(ty: &PlanType, mutable: bool) -> PlanType {
    if let PlanType::Generator(leaf, depth) = ty {
        if mutable {
            return PlanType::GeneratorMutRef(leaf.clone(), *depth);
        }
        return ty.clone();
    }
    match list_shape(ty) {
        Some((leaf, depth)) => {
            if mutable {
                PlanType::ListMutRef(leaf, depth)
            } else {
                PlanType::ListRef(leaf, depth)
            }
        }
        None => collection_reference(ty, mutable),
    }
}

/// A source parameter's frame storage: the collection and iterable references above, and for a `mut` text, model,
/// enum or tuple parameter the exclusive borrow of the caller's value, which is how legacy passes it.
pub fn parameter_storage_type(ty: &PlanType, mutable: bool) -> PlanType {
    if is_exclusive_borrow(ty) {
        return ty.clone();
    }
    if mutable {
        match ty {
            PlanType::String => return PlanType::StringMutRef,
            PlanType::Model(owner, name) => return PlanType::ModelMutRef(*owner, name.clone()),
            PlanType::Enum(owner, name) => return PlanType::EnumMutRef(*owner, name.clone()),
            PlanType::Tuple(elements) => return PlanType::TupleMutRef(elements.clone()),
            _ => {}
        }
    }
    list_parameter_type(ty, mutable)
}

/// The exclusive reference that changes a value of this type where it lives.
pub fn exclusive_reference(ty: &PlanType) -> Option<PlanType> {
    if matches!(ty, PlanType::Generator(..)) {
        return None;
    }
    let exclusive = parameter_storage_type(ty, true);
    if exclusive == *ty || exclusive == list_parameter_type(ty, false) {
        return None;
    }
    Some(exclusive)
}

pub fn is_mutable_list_reference(ty: &PlanType) -> bool {
    matches!(
        ty,
        PlanType::SetMutRef(_) | PlanType::DictMutRef(..) | PlanType::ListMutRef(..)
    )
}

/// Whether a value is the exclusive borrow a `mut` parameter receives, which moves rather than copies.
pub fn is_exclusive_borrow(ty: &PlanType) -> bool {
    if is_mutable_list_reference(ty) {
        return true;
    }
    matches!(
        ty,
        PlanType::GeneratorMutRef(..)
            | PlanType::StringMutRef
            | PlanType::ModelMutRef(..)
            | PlanType::EnumMutRef(..)
            | PlanType::TupleMutRef(_)
            | PlanType::NativeArrayMutRef(..)
    )
}

/// Consuming materializations against this whole frame's polls, including same-frame nested blocks.
pub fn taking_iterator_locals(block: &Block) -> Vec<i64> {
    taking_block_locals(block, &list_iterator_locals(block))
}

fn taking_block_locals(block: &Block, iterators: &[i64]) -> Vec<i64> {
    let mut result = Vec::new();
    for statement in &block.stmts {
        result.extend(taking_statement_locals(&statement.kind, iterators));
    }
    result
}

fn taking_statement_locals(kind: &StatementKind, iterators: &[i64]) -> Vec<i64> {
    let mut result = Vec::new();
    match kind {
        StatementKind::Assign { place, rvalue } => match rvalue {
            Rvalue::Use(Operand::Place(read)) => {
                if read.fact == OwnershipFact::Move
                    && let Some(local) = place.local_id()
                {
                    let index = local.index() as i64 + 1;
                    if iterators.contains(&index) {
                        result.push(index);
                    }
                }
            }
            Rvalue::Match { arms, .. } => {
                for arm in arms {
                    collect_arm_taking_locals(arm, iterators, &mut result);
                }
            }
            _ => {}
        },
        StatementKind::UnsafeRegion { body, .. } => result.extend(taking_block_locals(body, iterators)),
        StatementKind::Race { arms, .. } => {
            for arm in arms {
                result.extend(taking_block_locals(&arm.body, iterators));
            }
        }
        StatementKind::Loop { body } => result.extend(taking_block_locals(body, iterators)),
        StatementKind::If {
            then_block, else_block, ..
        } => {
            result.extend(taking_block_locals(then_block, iterators));
            if let Some(body) = else_block {
                result.extend(taking_block_locals(body, iterators));
            }
        }
        _ => {}
    }
    result
}

fn collect_arm_iterator_locals(arm: &MatchArm, result: &mut Vec<i64>) {
    for statement in arm.guard_stmts.iter().chain(&arm.body_stmts) {
        result.extend(statement_iterator_locals(&statement.kind));
    }
}

fn collect_arm_taking_locals(arm: &MatchArm, iterators: &[i64], result: &mut Vec<i64>) {
    for statement in arm.guard_stmts.iter().chain(&arm.body_stmts) {
        result.extend(taking_statement_locals(&statement.kind, iterators));
    }
}
