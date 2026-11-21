//! Borrowed collection equality uses the same PartialEq operations as legacy emission.

use incan_mir_plan_native::{Operand, PlanType, SourceSpan};

use crate::builtins::list_call;
use crate::collections::{collection_call, collection_owned, collection_reference, is_collection};
use crate::frame::{Builder, read_place, temporary};
use crate::lists::{list_element, list_shape};

/// Normalize shared collection parameters to the same owned layout as literal operands.
pub fn equality_shape(ty: &PlanType) -> Option<PlanType> {
    if let Some((leaf, depth)) = list_shape(ty) {
        return Some(PlanType::List(leaf, depth));
    }
    if is_collection(ty) {
        return Some(collection_owned(ty));
    }
    None
}

/// Instantiate an exact checked collection shape without cloning either operand or its elements.
pub fn emit_collection_equality(
    builder: &mut Builder,
    ty: &PlanType,
    references: Vec<Operand>,
    unequal: bool,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let output = temporary(builder, PlanType::Bool, span);
    let operation = if unequal { "unequal" } else { "equal" };
    match list_shape(ty) {
        Some((leaf, depth)) => {
            let reference = PlanType::ListRef(leaf.clone(), depth);
            list_call(
                builder,
                &format!("{operation}_list"),
                list_element(&leaf, depth),
                vec![reference.clone(), reference],
                PlanType::Bool,
                references,
                output.clone(),
                span,
            );
        }
        None => {
            let reference = collection_reference(ty, false);
            collection_call(
                builder,
                ty,
                operation,
                vec![reference.clone(), reference],
                PlanType::Bool,
                references,
                output.clone(),
                span,
            )?;
        }
    }
    Ok(read_place(output, false, span))
}
