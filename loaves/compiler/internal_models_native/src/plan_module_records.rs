//! Plan-module records: every body of a module with its plan-function record.

pub const PLAN_MODULE_FAMILY: &str = "plan-module";
pub const PLAN_MODULE_PAGE: &str = ".incnmodule";
pub const POSITIONED_MODULE_PAGE: &str = ".incnmodule.positioned";
pub const REPOSITION_MODULE_PAGE: &str = ".incnmodule.reposition";
pub const ADMITTED_MODULE_NOTE: &str = ".admitted";
pub const PARTIAL_MODULE_NOTE: &str = ".partially-admitted";

/// One body of a module with a plan product: the owner its plan-function record is keyed by, the canonical identity
/// of the body, its native function name, the key of its product, and the final lowering event with the product.
#[derive(Clone, PartialEq, Eq)]
pub struct PlanModuleFunction {
    pub identity: String,
    pub symbol: String,
    pub function: String,
    pub key: String,
    pub event: Vec<u8>,
}
crate::incan_debug::incan_debug!(PlanModuleFunction {
    identity,
    symbol,
    function,
    key,
    event
});

/// A module, named as lowering names it, with the plan function of every one of its bodies.
#[derive(Clone, PartialEq, Eq)]
pub struct PlanModuleValue {
    pub module: String,
    pub functions: Vec<PlanModuleFunction>,
}
crate::incan_debug::incan_debug!(PlanModuleValue { module, functions });
