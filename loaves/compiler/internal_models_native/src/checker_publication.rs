//! Typed admission and restoration counts published by one successful checked compiler invocation.

use std::collections::HashMap;

/// Permanent checker-record admission counts emitted by one compiler activation.
///
/// Every field has a default: zero counts, the summary `"0/0 reusable declarations"`, and empty collections.
/// `restored` names owners whose checked bodies the compiler restored from the selected records instead of checking
/// them. A selected record the compiler refused is not here: that body was checked afresh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckerRecordCounts {
    pub reusable_declarations: i64,
    pub all_declarations: i64,
    pub summary: String,
    pub artifact_refusals: HashMap<String, i64>,
    pub restored: Vec<String>,
    pub restored_body_ir: Vec<String>,
}

impl Default for CheckerRecordCounts {
    /// The reference model's field defaults.
    fn default() -> Self {
        Self {
            reusable_declarations: 0,
            all_declarations: 0,
            summary: "0/0 reusable declarations".to_owned(),
            artifact_refusals: HashMap::new(),
            restored: Vec::new(),
            restored_body_ir: Vec::new(),
        }
    }
}
