//! Counts observed by one graph-backed check, returned to Oven separately from public diagnostics.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{decode_graph_row, encode_graph_row, integer, number};
use internal_models_bindings::Description;

const COUNTS: &[u8] = b"chkbdy\x00";

/// Top-level bodies checked afresh or restored, and the pages physically decoded during this check. Every count
/// defaults to zero.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct CheckedBodyCounts {
    pub checked: i64,
    pub reused: i64,
    pub decoded: i64,
}
crate::incan_debug::incan_debug!(CheckedBodyCounts {
    checked,
    reused,
    decoded
});

fn description() -> Description {
    model_description(
        COUNTS,
        "incan.checked-body-counts/1",
        &[
            ModelField::new(b"check_", "checked", "i64"),
            ModelField::new(b"decode", "decoded", "i64"),
            ModelField::new(b"reuse_", "reused", "i64"),
        ],
    )
}

fn valid(counts: &CheckedBodyCounts) -> bool {
    counts.checked >= 0 && counts.reused >= 0 && counts.decoded >= 0
}

/// Write the three independent counts without copying module names or checked declarations.
pub fn encode_checked_body_counts(counts: &CheckedBodyCounts) -> Result<Vec<u8>, String> {
    if !valid(counts) {
        return Err("checked body counts must be nonnegative".into());
    }
    encode_graph_row(
        COUNTS,
        &description(),
        vec![
            required_value(b"check_", integer(counts.checked)),
            required_value(b"decode", integer(counts.decoded)),
            required_value(b"reuse_", integer(counts.reused)),
        ],
    )
}

/// Require an authentic typed counts row before reporting work performed by the compiler.
pub fn decode_checked_body_counts(encoded: &[u8]) -> Result<CheckedBodyCounts, String> {
    let fields = decode_graph_row(encoded, COUNTS, &description(), 3)?;
    let counts = CheckedBodyCounts {
        checked: number(&fields[0].value)?,
        decoded: number(&fields[1].value)?,
        reused: number(&fields[2].value)?,
    };
    if !valid(&counts) {
        return Err("checked body counts must be nonnegative".into());
    }
    Ok(counts)
}
