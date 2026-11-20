//! Lowering events, refusals, native interfaces and admitted definitions published by one lowering request.

pub const INTERFACE_REPORT: &str = "native-interface.crumb";
pub const ADMITTED_DEFINITIONS: &str = "admitted-definitions.crumb";

/// A named ordinal's position in the current model or enum table.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EventReference {
    pub kind: String,
    pub index: i64,
    pub name: String,
}

/// A Body IR node's current source coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EventPosition {
    pub line: i64,
    pub column: i64,
}

/// The exact published product and its use in this request.
///
/// Defaults: `admitted` false, `module`, `role` and `demand` empty, `runtime_requirements` zero, `references` and
/// `positions` empty.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// An explicit per-function reason lowering could not publish a reusable product.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweringRefusal {
    pub identity: String,
    pub function: String,
    pub refusal: String,
}

/// The standalone entry key and its complete loss-checked native plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeInterface {
    pub entry: String,
    pub plan: Vec<u8>,
}

/// A restored function's authenticated native definition inputs.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AdmittedDefinition {
    pub function: String,
    pub identity: String,
    pub key: String,
    pub digest: String,
}

/// Current Body IR node coordinates for an admitted scalar in a physically edited file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedPlacement {
    pub key: String,
    pub positions: Vec<EventPosition>,
}

/// A restored function's identity and native declaration positions in this request. `placement` defaults to `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedFunction {
    pub function: String,
    pub identity: String,
    pub references: Vec<EventReference>,
    pub placement: Option<AdmittedPlacement>,
}

/// Actual restored functions of a named module, including whether every ordinary body was restored. `inspected`
/// defaults to false.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedModule {
    pub module: String,
    pub complete: bool,
    pub functions: Vec<AdmittedFunction>,
    pub inspected: bool,
}
