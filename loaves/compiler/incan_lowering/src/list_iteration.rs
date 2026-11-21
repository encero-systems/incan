//! List iteration: cursor storage, snapshots of frozen collections, and the next-element protocol.

use incan_mir_plan_native::{ListLeaf, Place, PlanType, SourceSpan};
use incan_semantics_core::body_ir::{Place as SourcePlace, PlaceElem};

use crate::checked_values::scalar_place;
use crate::frame::Builder;
use crate::layout_types::collection_leaf;
use crate::list_values::list_base_place;
use crate::models::field_step_type;
use crate::pending::untranslated;
use crate::tuples::is_tuple_type;

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

/// Borrow whole storage or an exact field, payload or tuple element path.
///
/// A constant index selects a tuple element exactly like a structural field, so it stays on this path. Indexing a list
/// retains its separate element-materialization route.
pub fn list_root_place(builder: &mut Builder, value: &SourcePlace, span: &SourceSpan) -> Result<Place, String> {
    if value.projection.is_empty() {
        return list_base_place(builder, value, span);
    }
    if value.global().is_some() {
        return Err(untranslated("static_storage::computed_static_root"));
    }
    let Some(local) = value.local_id() else {
        return Err("unsupported Body IR global list root projection".into());
    };
    let mut ty = builder.locals[(local.index() as i64 + 1 + builder.local_offset) as usize]
        .ty
        .clone();
    // A receiver reference projects its model's fields.
    ty = match ty {
        PlanType::ModelRef(owner, name) | PlanType::ModelMutRef(owner, name) => PlanType::Model(owner, name),
        PlanType::EnumMutRef(owner, name) => PlanType::Enum(owner, name),
        other => other,
    };
    for projection in &value.projection {
        match projection {
            PlaceElem::Field { .. } | PlaceElem::OptionPayload { .. } | PlaceElem::UnionMember { .. } => {}
            PlaceElem::Index(_) => {
                if !is_tuple_type(&ty) {
                    return Err(list_projection_refusal(&builder.name, &ty, projection));
                }
            }
            _ => return Err(list_projection_refusal(&builder.name, &ty, projection)),
        }
        ty = field_step_type(builder.tables(), &ty, projection)?;
    }
    scalar_place(builder, value, span)
}

/// Name the retained carrier and projection that cannot provide structural collection storage.
fn list_projection_refusal(name: &str, ty: &PlanType, projection: &PlaceElem) -> String {
    format!("unsupported Body IR list root projection in {name}: {ty:?}; {projection:?}")
}
