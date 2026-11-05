//! The active plan of one compilation, with the lookups MIR construction makes by name.

use super::*;
use rustc_hir::def::DefKind;
use rustc_hir::def_id::DefId;
use std::collections::HashMap;
use std::sync::OnceLock;

/// One compilation's plan and its name index. Planned functions are indexed when the plan becomes active. The
/// session's source-local function items are indexed at their first lookup, because their identities exist only in
/// that compilation's session. Each index keeps the first entry of a name, in plan order and crate item order, as the
/// scans it replaces selected.
pub(crate) struct ActivePlan {
    plan: Arc<Plan>,
    by_name: HashMap<String, usize>,
    named_items: OnceLock<HashMap<String, DefId>>,
}

impl ActivePlan {
    /// Index the plan's functions by name.
    pub(super) fn new(plan: Arc<Plan>) -> Self {
        let mut by_name = HashMap::new();
        for (position, function) in plan.functions.iter().enumerate() {
            by_name.entry(function.name.to_string()).or_insert(position);
        }
        Self {
            plan,
            by_name,
            named_items: OnceLock::new(),
        }
    }

    /// The first planned function named `name`.
    pub(super) fn function(&self, name: &str) -> Option<&crate::plan::Function> {
        self.by_name.get(name).map(|position| &self.plan.functions[*position])
    }

    /// The first source-local function item named `name` among the session's crate items.
    pub(crate) fn item(&self, tcx: TyCtxt<'_>, name: &str) -> Option<DefId> {
        self.named_items
            .get_or_init(|| {
                let mut items = HashMap::new();
                for item in tcx.hir_crate_items(()).free_items() {
                    let definition = item.owner_id.to_def_id();
                    if tcx.def_kind(definition) == DefKind::Fn
                        && let Some(symbol) = tcx.opt_item_name(definition)
                    {
                        items.entry(symbol.as_str().to_owned()).or_insert(definition);
                    }
                }
                items
            })
            .get(name)
            .copied()
    }
}

impl std::ops::Deref for ActivePlan {
    type Target = Plan;

    fn deref(&self) -> &Plan {
        &self.plan
    }
}
