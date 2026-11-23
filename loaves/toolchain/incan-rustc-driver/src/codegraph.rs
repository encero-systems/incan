//! Graph export entry for an Oven-described Incan unit. Extraction remains in the shared compiler exporter.

use std::collections::BTreeMap;
use std::path::Path;

use incan_codegraph::CodegraphRecord;
use incan_driver::inspect::codegraph::collect_codegraph_with_interface;
use incan_driver::unit::UnitDescription;

/// Write a unit's portable binary graph and, when it checks, its checking-only library interface. Oven reads these
/// compiler-owned facts without reinterpreting source semantics.
///
/// The export is tolerant (RFC 106, "Checked and tolerant export"): a unit with checker diagnostics still writes its
/// parseable facts, marked degraded and without checked identities, together with an `OUTPUT.unchecked` note that
/// names each diagnostic, and publishes no interface. A unit the compiler cannot export at all writes the note alone.
/// Whether either refuses the graph is the caller's decision, so one cache entry serves strict and tolerant callers
/// alike; only an abnormal exit is a failure of this command.
pub fn incan(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    crate::native_timing::observe_frontend(false);
    if arguments.len() < 3 || arguments.len() > 4 {
        return Err("usage: --codegraph UNIT SYSROOT OUTPUT [INTERFACE]".into());
    }
    let [unit_path, sysroot, output] = &arguments[..3] else {
        return Err("missing codegraph arguments".into());
    };
    if let Err(error) = export(unit_path, sysroot, output, arguments.get(3)) {
        let reason = error.to_string().replace('\n', " ");
        std::fs::write(format!("{output}.unchecked"), format!("1 compiler refusal\n- {reason}"))?;
    }
    Ok(())
}

/// Export one unit's records, then write its interface, note and graph, the graph last so an existing graph is whole.
fn export(
    unit_path: &str,
    sysroot: &str,
    output: &str,
    interface_path: Option<&String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let unit = UnitDescription::read(Path::new(unit_path)).map_err(|error| error.message)?;
    let rust = crate::frontend::dependencies(&unit, &[])?.rust_facts(&unit, Path::new(sysroot));
    let (records, interface) =
        collect_codegraph_with_interface(&unit.source, std::slice::from_ref(&unit), true, Some(&rust))?;
    let note = unchecked_note(&records, &unit.project_root);
    if let Some(path) = interface_path {
        match (interface, &note) {
            (Some(manifest), _) => std::fs::write(path, manifest.to_json_string()?)?,
            (None, Some(_)) => {}
            (None, None) => return Err("a checked interface requires a library unit".into()),
        }
    }
    if let Some(note) = note {
        std::fs::write(format!("{output}.unchecked"), note)?;
    }
    std::fs::write(
        output,
        incan_codegraph::artifact::encode_graph(&records, &unit.project_root)?,
    )?;
    Ok(())
}

/// Name a tolerant export's diagnostics for Oven: the count, then one line per diagnostic with its code, message and
/// project-relative location. `None` when the unit checked and every fact is checked.
fn unchecked_note(records: &[CodegraphRecord], project_root: &Path) -> Option<String> {
    let diagnostics = records
        .iter()
        .filter_map(|record| match record {
            CodegraphRecord::Diagnostic(diagnostic) => Some(diagnostic),
            _ => None,
        })
        .collect::<Vec<_>>();
    if diagnostics.is_empty() {
        return None;
    }
    let plural = if diagnostics.len() == 1 { "" } else { "s" };
    let mut note = format!("{} checker diagnostic{plural}", diagnostics.len());
    for diagnostic in diagnostics {
        let span = &diagnostic.primary_span;
        let file = Path::new(&span.file);
        let file = file.strip_prefix(project_root).unwrap_or(file);
        note.push_str(&format!(
            "\n- {}: {} ({}:{})",
            diagnostic.code,
            diagnostic.message.replace('\n', " "),
            file.display(),
            span.start_line
        ));
    }
    Some(note)
}

/// Decode a portable Loaf graph for a requested JSON projection. An optional per-kind field map trims transport after
/// wire validation; query semantics and the field selection remain in Oven.
pub fn read(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [path, root, group, ..] = arguments else {
        return Err("usage: --read-codegraph GRAPH ROOT GROUP [FIELDS_JSON]".into());
    };
    if arguments.len() > 4 {
        return Err("usage: --read-codegraph GRAPH ROOT GROUP [FIELDS_JSON]".into());
    }
    let fields = match arguments.get(3) {
        Some(document) => serde_json::from_str::<BTreeMap<String, Vec<String>>>(document)?,
        None => BTreeMap::new(),
    };
    let graph =
        incan_codegraph::artifact::GraphArtifact::decode(&std::fs::read(path)?).map_err(|error| error.to_string())?;
    use std::io::Write;
    let mut output = std::io::BufWriter::new(std::io::stdout().lock());
    let records = if group == "origins" {
        graph.origins()
    } else {
        graph.projected_records(Path::new(root), group, &fields)
    }
    .map_err(|error| error.to_string())?;
    for record in records {
        serde_json::to_writer(&mut output, &record)?;
        writeln!(output)?;
    }
    output.flush()?;
    Ok(())
}
