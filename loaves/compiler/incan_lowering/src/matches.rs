//! Enum matches and the owners live where their branches join.

use incan_mir_plan_native::{PlanType, SourceSpan};

use crate::collections::is_owned_collection;
use crate::frame::{Builder, drop_owned, finish_fallthrough};

/// Close each `if` branch or match arm reaching the join, keeping live only the owners every such branch left live.
///
/// Body IR counts last uses across branches in source order, so one branch may move a value that another only borrows,
/// and a branch's own temporaries (a condition's constructed model, a comparison's enum value) end with it. Rust drops
/// such a value at scope end through a drop flag. Destroying text, collections, nominal values and lazy generators
/// only releases their storage, so releasing the value at the end of the branch that kept it is equivalent. An owner
/// of any other type left live on only some paths stays refused.
pub fn join_branch_owners(
    builder: &mut Builder,
    join: i64,
    ends: &[i64],
    arm_owners: &[Vec<i64>],
    span: &SourceSpan,
) -> Result<Vec<i64>, String> {
    let mut live: Vec<i64> = Vec::new();
    if let Some(first) = arm_owners.first()
        && !ends.is_empty()
    {
        for local in first {
            if arm_owners.iter().all(|owned| owned.contains(local)) {
                live.push(*local);
            }
        }
    }
    for (end, owners) in ends.iter().zip(arm_owners) {
        builder.select(*end);
        builder.owned = owners.clone();
        for local in owners {
            if live.contains(local) {
                continue;
            }
            if !releases_only_memory(&builder.locals[*local as usize].ty) {
                return Err("unsupported Body IR owned-value branch merge".into());
            }
            drop_owned(builder, *local, span);
        }
        finish_fallthrough(builder, join, span);
    }
    Ok(live)
}

/// Report whether destroying an owner only releases storage, including a lazy generator's retained state.
fn releases_only_memory(ty: &PlanType) -> bool {
    *ty == PlanType::String
        || is_owned_collection(ty)
        || matches!(
            ty,
            PlanType::Generator(..) | PlanType::Model(..) | PlanType::Enum(..) | PlanType::Tuple(_)
        )
}
