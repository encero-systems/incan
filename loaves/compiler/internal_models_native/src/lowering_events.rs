//! Lowering events, refusals, native interfaces and admitted definitions published by one lowering request.

pub const INTERFACE_REPORT: &str = "native-interface.crumb";
pub const ADMITTED_DEFINITIONS: &str = "admitted-definitions.crumb";

/// A named ordinal's position in the current model or enum table.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct EventReference {
    pub kind: String,
    pub index: i64,
    pub name: String,
}
crate::incan_debug::incan_debug!(EventReference { kind, index, name });

/// A Body IR node's current source coordinates.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventPosition {
    pub line: i64,
    pub column: i64,
}
crate::incan_debug::incan_debug!(EventPosition { line, column });

/// The exact published product and its use in this request.
///
/// Defaults: `admitted` false, `module`, `role` and `demand` empty, `runtime_requirements` zero, `references` and
/// `positions` empty.
#[derive(Clone, PartialEq, Eq)]
pub struct LoweringEvent {
    pub identity: String,
    pub key: String,
    pub function: String,
    pub product: Vec<u8>,
    pub reused: bool,
    pub admitted: bool,
    pub module: String,
    pub role: String,
    pub demand: String,
    pub runtime_requirements: i64,
    pub references: Vec<EventReference>,
    pub positions: Vec<EventPosition>,
}
crate::incan_debug::incan_debug!(LoweringEvent {
    identity,
    key,
    function,
    product,
    reused,
    admitted,
    module,
    role,
    demand,
    runtime_requirements,
    references,
    positions
});

/// An explicit per-function reason lowering could not publish a reusable product.
#[derive(Clone, PartialEq, Eq)]
pub struct LoweringRefusal {
    pub identity: String,
    pub function: String,
    pub refusal: String,
}
crate::incan_debug::incan_debug!(LoweringRefusal {
    identity,
    function,
    refusal
});

/// The standalone entry key and its complete loss-checked native plan.
#[derive(Clone, PartialEq, Eq)]
pub struct NativeInterface {
    pub entry: String,
    pub plan: Vec<u8>,
}
crate::incan_debug::incan_debug!(NativeInterface { entry, plan });

/// A restored function's authenticated native definition inputs.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AdmittedDefinition {
    pub function: String,
    pub identity: String,
    pub key: String,
    pub digest: String,
}
crate::incan_debug::incan_debug!(AdmittedDefinition {
    function,
    identity,
    key,
    digest
});

/// Current Body IR node coordinates for an admitted scalar in a physically edited file.
#[derive(Clone, PartialEq, Eq)]
pub struct AdmittedPlacement {
    pub key: String,
    pub positions: Vec<EventPosition>,
}
crate::incan_debug::incan_debug!(AdmittedPlacement { key, positions });

/// A restored function's identity and native declaration positions in this request. `placement` defaults to `None`.
#[derive(Clone, PartialEq, Eq)]
pub struct AdmittedFunction {
    pub function: String,
    pub identity: String,
    pub references: Vec<EventReference>,
    pub placement: Option<AdmittedPlacement>,
}
crate::incan_debug::incan_debug!(AdmittedFunction {
    function,
    identity,
    references,
    placement
});

/// Actual restored functions of a named module, including whether every ordinary body was restored. `inspected`
/// defaults to false.
#[derive(Clone, PartialEq, Eq)]
pub struct AdmittedModule {
    pub module: String,
    pub complete: bool,
    pub functions: Vec<AdmittedFunction>,
    pub inspected: bool,
}
crate::incan_debug::incan_debug!(AdmittedModule {
    module,
    complete,
    functions,
    inspected
});
