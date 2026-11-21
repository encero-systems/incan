//! List iteration: cursor storage, snapshots of frozen collections, and the next-element protocol.

use incan_mir_plan_native::{ListLeaf, PlanType};

use crate::layout_types::collection_leaf;

/// Keep ordinary elements exact while allowing membership to compare owned text with frozen text.
pub fn frozen_list_snapshot_type(ty: &PlanType, text_membership: bool) -> Result<PlanType, String> {
    let PlanType::FrozenCollection(name, elements) = ty else {
        return Err("unsupported Body IR nonfrozen snapshot".into());
    };
    if !matches!(name.as_str(), "FrozenList" | "FrozenSet" | "FrozenDict") || elements.is_empty() {
        return Err("unsupported Body IR frozen collection snapshot".into());
    }
    let element = &elements[0];
    if *element == PlanType::StaticText || (text_membership && *element == PlanType::FrozenStr) {
        return Ok(PlanType::List(ListLeaf::Str, 1));
    }
    if *element == PlanType::StaticBytes {
        if name != "FrozenList" {
            return Err("unsupported Body IR frozen byte set or dictionary snapshot".into());
        }
        return Ok(PlanType::List(ListLeaf::U8, 2));
    }
    if let PlanType::Model(..) | PlanType::FrozenCollection(..) = element {
        if name != "FrozenList" {
            return Err("unsupported Body IR frozen nominal set or dictionary snapshot".into());
        }
        return Ok(PlanType::List(collection_leaf(element)?, 1));
    }
    if !matches!(
        element,
        PlanType::Int | PlanType::Float | PlanType::Bool | PlanType::FrozenStr | PlanType::FrozenBytes
    ) {
        return Err("unsupported Body IR frozen collection snapshot with nonprimitive elements".into());
    }
    Ok(PlanType::List(collection_leaf(element)?, 1))
}
