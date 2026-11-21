//! Release literal owners materialized only to borrow text during one runtime call.

use incan_mir_plan_native::SourceSpan;

use crate::frame::{Builder, drop_owned};

/// Drop newly materialized argument owners after the borrow ends, retaining the call result and incoming owners.
pub fn drop_borrowed_text_temporaries(builder: &mut Builder, incoming: &[i64], result: i64, span: &SourceSpan) {
    let created: Vec<i64> = builder
        .owned
        .iter()
        .copied()
        .filter(|owner| !incoming.contains(owner) && *owner != result)
        .collect();
    for owner in created.into_iter().rev() {
        drop_owned(builder, owner, span);
    }
}
