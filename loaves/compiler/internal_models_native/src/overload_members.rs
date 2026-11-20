//! Overload member pages and the method lookups a checked reader performed.

use std::collections::{HashMap, HashSet};

pub const OVERLOAD_MEMBERS: &str = "overload-members";
pub const LOOKUP_FILE: &str = "method-lookups.crumb";

/// One method lookup a checked reader performed and the candidates it selected among.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct MethodLookup {
    pub reader: String,
    pub module: String,
    pub receiver: String,
    pub method: String,
    pub candidates: Vec<String>,
}
crate::incan_debug::incan_debug!(MethodLookup {
    reader,
    module,
    receiver,
    method,
    candidates
});

/// Method lookups grouped by reader, and the readers retained without lookups. Both default to empty.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct MethodReads {
    pub groups: HashMap<String, Vec<MethodLookup>>,
    pub retained: HashSet<String>,
}
crate::incan_debug::incan_debug!(MethodReads { groups, retained });
