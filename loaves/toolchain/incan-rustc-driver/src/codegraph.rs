//! Graph export entry for an Oven-described Incan unit. Extraction remains in the shared compiler exporter.

use std::path::Path;

use incan_driver::inspect::codegraph::collect_codegraph_with_interface;
use incan_driver::unit::UnitDescription;

/// Write a checked unit's portable binary graph and optional checking-only library interface. Oven reads these
/// compiler-owned facts without reinterpreting source semantics.
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
    std::fs::write(
        output,
        incan_codegraph::artifact::encode_graph(&records, &unit.project_root)?,
    )?;
    Ok(())
}

/// Decode a portable Loaf graph for a requested JSON query projection; semantic selection remains in Oven.
pub fn read(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [path, root, group] = arguments else {
        return Err("usage: --read-codegraph GRAPH ROOT GROUP".into());
    };
    let graph =
        incan_codegraph::artifact::GraphArtifact::decode(&std::fs::read(path)?).map_err(|error| error.to_string())?;
    use std::io::Write;
    let mut output = std::io::BufWriter::new(std::io::stdout().lock());
    let records = if group == "origins" {
        graph.origins()
    } else {
        graph.records(Path::new(root), group)
    }
    .map_err(|error| error.to_string())?;
    for record in records {
        serde_json::to_writer(&mut output, &record)?;
        writeln!(output)?;
    }
    output.flush()?;
    Ok(())
}
