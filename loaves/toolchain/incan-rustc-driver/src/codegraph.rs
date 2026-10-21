//! Graph export entry for an Oven-described Incan unit. Extraction remains in the shared compiler exporter.

use std::path::Path;

use incan_driver::inspect::codegraph::collect_codegraph_records_with_rust;
use incan_driver::unit::UnitDescription;

/// Write the checked graph as JSONL; Oven caches and queries these records without reinterpreting source semantics.
pub fn incan(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [unit_path, sysroot, output] = arguments else {
        return Err("usage: --codegraph UNIT SYSROOT OUTPUT".into());
    };
    let unit = UnitDescription::read(Path::new(unit_path)).map_err(|error| error.message)?;
    let rust = crate::frontend::dependencies(&unit, &[])?.rust_facts(Path::new(sysroot));
    let records = collect_codegraph_records_with_rust(&unit.source, std::slice::from_ref(&unit), false, Some(&rust))?;
    std::fs::write(output, incan_codegraph::to_jsonl(&records)?)?;
    Ok(())
}
