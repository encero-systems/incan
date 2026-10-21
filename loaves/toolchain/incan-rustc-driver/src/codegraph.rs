//! Graph export entry for an Oven-described Incan unit. Extraction remains in the shared compiler exporter.

use std::path::Path;

use incan_driver::inspect::codegraph::collect_codegraph_with_interface;
use incan_driver::unit::UnitDescription;

/// Write the checked graph as JSONL; Oven caches and queries these records without reinterpreting source semantics.
pub fn incan(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.len() < 3 || arguments.len() > 4 {
        return Err("usage: --codegraph UNIT SYSROOT OUTPUT [INTERFACE]".into());
    }
    let [unit_path, sysroot, output] = &arguments[..3] else {
        return Err("missing codegraph arguments".into());
    };
    let unit = UnitDescription::read(Path::new(unit_path)).map_err(|error| error.message)?;
    let rust = crate::frontend::dependencies(&unit, &[])?.rust_facts(Path::new(sysroot));
    let (records, interface) =
        collect_codegraph_with_interface(&unit.source, std::slice::from_ref(&unit), false, Some(&rust))?;
    if let Some(path) = arguments.get(3) {
        let manifest = interface.ok_or("a checked interface requires a library unit")?;
        std::fs::write(path, manifest.to_json_string()?)?;
    }
    std::fs::write(output, incan_codegraph::to_jsonl(&records)?)?;
    Ok(())
}
