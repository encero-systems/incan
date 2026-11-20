//! Plan-function records: the lowering inputs, native function payload, instance requests and payload digest.

use crate::plan_wire_rows::PreparedWireReader;
use internal_models_bindings::Value;
use std::collections::HashMap;

/// The exact lowering inputs, native function payload, instance requests and payload digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanFunctionRecord {
    pub inputs: Vec<String>,
    pub function: Vec<u8>,
    pub requests: Vec<Vec<u8>>,
    pub digest: String,
}

/// One publication's decoder and bounded cache of fully authenticated runtime signature lists. Starts empty.
#[derive(Default)]
pub struct LoweringDemands {
    pub(crate) reader: Option<PreparedWireReader>,
    pub(crate) externals: HashMap<String, Vec<Value>>,
    pub(crate) cache_bytes: i64,
}
