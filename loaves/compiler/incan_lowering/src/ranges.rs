//! Integer range values in source bindings and temporaries.

use incan_semantics_core::body_ir::LocalOrigin;
use incan_semantics_core::{IncanPrimitiveType, IncanType};

/// Admit an integer range only in a source binding or temporary, never at a signature or capture boundary.
pub fn range_value_local(ty: &IncanType, origin: &LocalOrigin) -> bool {
    if !matches!(origin, LocalOrigin::UserBinding | LocalOrigin::Temporary) {
        return false;
    }
    integer_range(ty)
}

/// The checker's `Range[int]` value type.
fn integer_range(ty: &IncanType) -> bool {
    match ty {
        IncanType::Generic { base, args } if base == "Range" && args.len() == 1 => {
            matches!(args[0], IncanType::Primitive(IncanPrimitiveType::Int))
        }
        _ => false,
    }
}
