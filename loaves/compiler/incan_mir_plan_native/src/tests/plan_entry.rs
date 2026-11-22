//! Retained entry records bind the exact selected generation, entry and ordered lowering inputs.

use crate::records::plan_function::{encode_entry_record, verify_entry_record};

/// Every input, the entry name and the exact envelope bytes take part in an entry record's verification.
#[test]
fn entry_validation_depends_on_all_current_inputs() -> Result<(), String> {
    let inputs: Vec<String> = ["incan.plan-entry/2", "toolchain", "main", "implicit-main", "callee-key"]
        .iter()
        .map(|input| (*input).to_owned())
        .collect();
    let record = encode_entry_record("main", &inputs)?;
    verify_entry_record(&record, "main", &inputs)?;
    assert!(verify_entry_record(&record, "other", &inputs).is_err());
    for index in 0..inputs.len() {
        let mut changed = inputs.clone();
        changed[index] = "different".to_owned();
        assert!(verify_entry_record(&record, "main", &changed).is_err());
    }
    let mut trailing = record.clone();
    trailing.push(120);
    assert!(verify_entry_record(&trailing, "main", &inputs).is_err());
    assert!(verify_entry_record(b"invalid", "main", &inputs).is_err());
    Ok(())
}
