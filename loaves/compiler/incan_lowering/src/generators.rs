//! Lazy generator layouts, producer frames and their runtime operations.

use incan_mir_plan_native::{ListLeaf, PlanType};
use incan_semantics_core::IncanType;

use crate::collections::{leaf_type, primitive_leaf};
use crate::tuples::{generic_tuple_type, tuple_type, type_name};

/// Admit primitive generators and flat pair iterators through their checked element layouts.
pub fn generator_type(ty: &IncanType) -> Option<Result<PlanType, String>> {
    let IncanType::Generic { base, args } = ty else {
        return None;
    };
    if base != "Generator" && base != "Iterator" {
        return None;
    }
    if args.len() != 1 {
        return Some(Err(format!("unsupported Body IR type {}", type_name(ty))));
    }
    if base == "Iterator"
        && let Ok(PlanType::Tuple(elements)) = iterator_pair_type(&args[0])
        && elements.len() == 2
    {
        return Some(Ok(PlanType::ZipIterator(elements[0].clone(), elements[1].clone())));
    }
    match generator_item(&args[0]) {
        Ok((leaf, depth)) => Some(Ok(PlanType::Generator(leaf, depth))),
        Err(_) => Some(Err(format!("unsupported Body IR type {}", type_name(ty)))),
    }
}

/// Lay out a primitive item, or a list nesting one, such as the lists `batch` yields.
fn generator_item(ty: &IncanType) -> Result<(ListLeaf, i64), String> {
    if let IncanType::Generic { base, args } = ty
        && base == "List"
        && args.len() == 1
    {
        let (leaf, depth) = generator_item(&args[0])?;
        return Ok((leaf, depth + 1));
    }
    Ok((primitive_leaf(ty, false)?, 0))
}

fn iterator_pair_type(ty: &IncanType) -> Result<PlanType, String> {
    match ty {
        IncanType::Tuple(elements) => tuple_type(elements),
        IncanType::Generic { .. } => generic_tuple_type(ty),
        _ => Err("unsupported Body IR iterator element".into()),
    }
}

/// A lazy generator or its mutable reference, never a materialized list.
pub fn generator_shape(ty: &PlanType) -> Option<(ListLeaf, i64)> {
    match ty {
        PlanType::Generator(leaf, depth) | PlanType::GeneratorMutRef(leaf, depth) => Some((leaf.clone(), *depth)),
        _ => None,
    }
}

pub fn is_generator_type(ty: &PlanType) -> bool {
    matches!(ty, PlanType::Generator(..))
}

pub fn is_generator_reference(ty: &PlanType) -> bool {
    matches!(ty, PlanType::GeneratorMutRef(..))
}

/// The generator's exact yielded type; depth zero is its immediate retained leaf.
pub fn generator_element(leaf: &ListLeaf, depth: i64) -> PlanType {
    if depth == 0 {
        return leaf_type(leaf);
    }
    PlanType::List(leaf.clone(), depth)
}
