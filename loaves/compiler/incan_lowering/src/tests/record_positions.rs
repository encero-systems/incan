//! A plan product holds no source line: moved lines inside a body change only its request-local position map.
//!
//! The reference's fourth test, finalization against every carried frame, waits on `scalar_store::finalize_scalar`,
//! which records publication brings.

use std::path::{Path, PathBuf};

use incan_mir_plan_native::{
    Function, Local, PlanType, SourceSpan, TerminatorKind, block, decode_plan_function_record, encode_product,
    source_span, unit_nominals,
};

use crate::record_positions::{
    index_positions, indexed_product, place_positions, positioned_product, relocate_product,
};
use crate::scalar_store::{record_scalar_inputs, retain_scalar, retained_scalar};

/// A function whose block starts on one body line and whose return sits on another.
fn positioned(name: &str, owner: &SourceSpan, statement_line: i64, exit_line: i64) -> Function {
    let block_span = source_span("positions.incn", statement_line, 5);
    let exit_span = source_span("positions.incn", exit_line, 5);
    let locals = vec![Local {
        ty: PlanType::Unit,
        span: owner.clone(),
    }];
    let mut exit = block(Vec::new(), TerminatorKind::Return, &exit_span);
    exit.span = block_span;
    Function::new(
        name.to_owned(),
        Vec::new(),
        PlanType::Unit,
        locals,
        vec![exit],
        owner.clone(),
    )
}

/// A fresh private product directory for one test.
fn products(test: &str) -> Result<PathBuf, String> {
    let directory = std::env::temp_dir().join(format!("incan-lowering-positions-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    Ok(directory)
}

/// The single retained product record as its binary Crumb envelope.
fn only_product(products: &Path) -> Result<Vec<u8>, String> {
    for entry in std::fs::read_dir(products).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.to_string_lossy().ends_with(".incnfunction") {
            return std::fs::read(path).map_err(|error| error.to_string());
        }
    }
    Err("no plan product was retained".into())
}

/// A comment inserted inside a body moves its second node down one line.
///
/// The record key and the stored product bytes stay the same, and restoring the product places its spans at the
/// nodes' new lines.
#[test]
fn moved_body_lines_keep_the_plan_product() -> Result<(), String> {
    let directory = products("moved")?;
    let text = directory.to_string_lossy().into_owned();
    let owner = source_span("positions.incn", 10, 1);
    let before_nodes = [(0, 1), (1, 5), (2, 5)];
    let before = positioned("relay", &owner, 11, 12);
    let inputs = record_scalar_inputs(
        "checked-meaning",
        &before.name,
        &owner,
        false,
        "reads",
        before_nodes.len(),
    );
    retain_scalar(&text, &inputs, &before, &[], &[], &before_nodes, &[], &[])?;
    let stored = only_product(&directory)?;
    let after_nodes = [(0, 1), (1, 5), (3, 5)];
    let record = decode_plan_function_record(&stored)?;
    let placed = place_positions(&record.function, &after_nodes)?;
    let moved = positioned_product(&record.function, &after_nodes, "moved.incn", 20, &mut Vec::new())?;
    assert!(moved == relocate_product(&placed, "moved.incn", 20)?);
    assert!(positioned_product(&record.function, &[], "moved.incn", 20, &mut Vec::new()).is_err());
    let after = positioned("relay", &owner, 11, 13);
    let unchanged = record_scalar_inputs(
        "checked-meaning",
        &after.name,
        &owner,
        false,
        "reads",
        after_nodes.len(),
    );
    assert!(unchanged == inputs);
    let unit = unit_nominals(Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let Some((function, _, _, _, _)) = retained_scalar(&text, &inputs, &unit, Some(&owner), &after_nodes) else {
        return Err("a comment that moved lines inside the body made the product a miss".into());
    };
    assert!(
        function == after,
        "the reused product kept the lines of the body it was captured from"
    );
    retain_scalar(&text, &inputs, &after, &[], &[], &after_nodes, &[], &[])?;
    assert!(
        only_product(&directory)? == stored,
        "moved lines changed the plan product's bytes"
    );
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

/// Indexing in one pass equals relocating then indexing, and refuses missing nodes and another file.
#[test]
fn indexed_product_matches_both_position_passes_and_refusals() -> Result<(), String> {
    let owner = source_span("positions.incn", 10, 1);
    let function = positioned("relay", &owner, 11, 12);
    let helper = positioned("helper", &owner, 12, 11);
    let wire = encode_product(&function, &[], &[], &[helper])?;
    let nodes = [(0, 1), (1, 5), (2, 5), (1, 5)];
    let relative = relocate_product(&wire, "<owner>", 1)?;
    assert!(indexed_product(&wire, &nodes, "positions.incn", 10)? == index_positions(&relative, &nodes)?);
    assert!(indexed_product(&wire, &[(0, 1)], "positions.incn", 10).is_err());
    assert!(indexed_product(&wire, &nodes, "other.incn", 10).is_err());
    Ok(())
}

/// A span below the owner line that no Body IR node carries refuses capture rather than go stale.
#[test]
fn unlocated_body_span_refuses_a_position_free_product() -> Result<(), String> {
    let directory = products("unlocated")?;
    let owner = source_span("positions.incn", 10, 1);
    let nodes = [(0, 1), (1, 5)];
    let function = positioned("relay", &owner, 11, 12);
    let inputs = record_scalar_inputs("checked-meaning", &function.name, &owner, false, "reads", nodes.len());
    let retained = retain_scalar(
        &directory.to_string_lossy(),
        &inputs,
        &function,
        &[],
        &[],
        &nodes,
        &[],
        &[],
    );
    let _ = std::fs::remove_dir_all(&directory);
    match retained {
        Ok(()) => Err("a span no node locates was retained with its line".into()),
        Err(message) => {
            assert!(message.contains("locates no Body IR node"), "{message}");
            Ok(())
        }
    }
}
