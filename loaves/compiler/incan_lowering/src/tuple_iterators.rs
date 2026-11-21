//! Zip iteration over tuple elements.

use incan_mir_plan_native::{PlanType, TupleElement};

/// Recognize a zip owner or its retained poll reference.
pub fn zip_shape(ty: &PlanType) -> Option<(TupleElement, TupleElement)> {
    match ty {
        PlanType::ZipIterator(left, right)
        | PlanType::ZipIteratorRef(left, right)
        | PlanType::ZipIteratorMutRef(left, right) => Some((left.clone(), right.clone())),
        _ => None,
    }
}
