//! Typed admission and restoration counts published by one successful checked compiler invocation.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{decode_graph_row, encode_graph_row, integer, items, number, text, tuple_items};
use internal_models_bindings::{Description, Value};
use std::collections::{HashMap, HashSet};

const REPORT: &[u8] = b"chkpub\x00";

/// Permanent checker-record admission counts emitted by one compiler activation.
///
/// Every field has a default: zero counts, the summary `"0/0 reusable declarations"`, and empty collections.
/// `restored` names owners whose checked bodies the compiler restored from the selected records instead of checking
/// them. A selected record the compiler refused is not here: that body was checked afresh.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckerRecordCounts {
    pub reusable_declarations: i64,
    pub all_declarations: i64,
    pub summary: String,
    pub artifact_refusals: HashMap<String, i64>,
    pub restored: Vec<String>,
    pub restored_body_ir: Vec<String>,
}
crate::incan_debug::incan_debug!(CheckerRecordCounts {
    reusable_declarations,
    all_declarations,
    summary,
    artifact_refusals,
    restored,
    restored_body_ir
});

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

/// The file suffix of the checker report a compiler activation writes.
pub fn checker_report_suffix() -> String {
    ".checker-report.crumb".to_owned()
}

fn description(previous: bool) -> Description {
    let mut fields = vec![
        ModelField::new(b"all___", "all_declarations", "i64"),
        ModelField::new(b"refuse", "artifact_refusals", "list<tuple<string,i64>>"),
        ModelField::new(b"restor", "restored", "list<string>"),
        ModelField::new(b"reuse_", "reusable_declarations", "i64"),
        ModelField::new(b"sumary", "summary", "string"),
    ];
    if previous {
        return model_description(REPORT, "incan.checker-reuse/2", &fields);
    }
    fields.push(ModelField::new(b"usedir", "restored_body_ir", "list<string>"));
    model_description(REPORT, "incan.checker-reuse/3", &fields)
}

fn valid(counts: &CheckerRecordCounts) -> bool {
    if counts.reusable_declarations < 0 || counts.reusable_declarations > counts.all_declarations {
        return false;
    }
    for owners in [&counts.restored, &counts.restored_body_ir] {
        let distinct: HashSet<&String> = owners.iter().collect();
        if owners.len() as i64 > counts.reusable_declarations
            || owners.iter().any(String::is_empty)
            || distinct.len() != owners.len()
        {
            return false;
        }
    }
    counts
        .artifact_refusals
        .iter()
        .all(|(name, count)| !name.is_empty() && *count > 0)
}

/// Encode the counts with refusal names in their sorted order.
pub fn encode_checker_counts(counts: &CheckerRecordCounts) -> Result<Vec<u8>, String> {
    if !valid(counts) {
        return Err("invalid checker publication counts".into());
    }
    let mut names: Vec<&String> = counts.artifact_refusals.keys().collect();
    names.sort();
    let ordered = names
        .into_iter()
        .map(|name| Value::Tuple(vec![Value::Text(name.clone()), integer(counts.artifact_refusals[name])]))
        .collect();
    let texts = |owners: &[String]| Value::List(owners.iter().cloned().map(Value::Text).collect());
    encode_graph_row(
        REPORT,
        &description(false),
        vec![
            required_value(b"all___", integer(counts.all_declarations)),
            required_value(b"refuse", Value::List(ordered)),
            required_value(b"restor", texts(&counts.restored)),
            required_value(b"reuse_", integer(counts.reusable_declarations)),
            required_value(b"sumary", Value::Text(counts.summary.clone())),
            required_value(b"usedir", texts(&counts.restored_body_ir)),
        ],
    )
}

/// Decode the current report, or the previous one without restored Body IR owners.
pub fn decode_checker_counts(encoded: &[u8]) -> Result<CheckerRecordCounts, String> {
    let fields = match decode_graph_row(encoded, REPORT, &description(false), 6) {
        Ok(current) => current,
        Err(_) => decode_graph_row(encoded, REPORT, &description(true), 5)?,
    };
    let mut refusals = HashMap::new();
    let mut previous = String::new();
    for value in items(&fields[1].value)? {
        let pair = tuple_items(value, 2)?;
        let name = text(&pair[0])?;
        if name <= previous {
            return Err("checker refusal names must be distinct and ordered".into());
        }
        previous = name.clone();
        refusals.insert(name, number(&pair[1])?);
    }
    let names = |value: &Value| items(value)?.iter().map(text).collect::<Result<Vec<_>, _>>();
    let restored = names(&fields[2].value)?;
    let restored_body_ir = if fields.len() == 6 {
        names(&fields[5].value)?
    } else {
        Vec::new()
    };
    let counts = CheckerRecordCounts {
        all_declarations: number(&fields[0].value)?,
        reusable_declarations: number(&fields[3].value)?,
        restored,
        restored_body_ir,
        summary: text(&fields[4].value)?,
        artifact_refusals: refusals,
    };
    if !valid(&counts) {
        return Err("invalid checker publication counts".into());
    }
    Ok(counts)
}
