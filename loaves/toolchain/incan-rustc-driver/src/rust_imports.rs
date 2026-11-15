//! Export Rust import syntax from the compiler's ordinary source graph for Oven dependency preparation, and, with
//! `--source-graph`, what that graph's collection read and probed, which Oven keys a program or test unit by.

use std::path::Path;

use incan_driver::modules::{collect_modules_detailed_with_session, collect_source_graph_reads};
use incan_driver::session::CompilationSession;
use incan_driver::unit::UnitDescription;
use incan_frontend::ast::{Declaration, ImportKind};
use incan_frontend::parsed_module::ParsedModule;

/// Print the source graph's Rust crate requests without checking Rust items or selecting artifacts. Oven resolves
/// these requests before checking; parsing uses the same module collection and dependency vocabulary as compilation.
///
/// `--rust-imports UNIT --source-graph` prints the files the collection read and the absent paths its imports looked
/// for instead, as `{"read": [...], "absent": [...], "complete": bool, "imports": [...]}`, with the same Rust crate
/// requests when collection completes, so one request answers both questions. A collection the compiler would refuse
/// still prints what it read and probed (`complete` is false, no `imports`): the refusal depends on exactly those
/// inputs.
pub fn export(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (path, source_graph) = match arguments {
        [path] => (path, false),
        [path, flag] if flag == "--source-graph" => (path, true),
        _ => return Err("usage: incan-rustc-driver --rust-imports UNIT [--source-graph]".into()),
    };
    let unit = UnitDescription::read(Path::new(path)).map_err(|error| error.message)?;
    let session = CompilationSession::for_formatting(&unit).map_err(|error| error.message)?;
    if source_graph {
        let (reads, collected) = collect_source_graph_reads(unit.source.clone(), &session);
        let mut report = serde_json::json!({
            "read": reads.read,
            "absent": reads.absent,
            "complete": collected.is_ok(),
        });
        if let Ok(modules) = collected {
            report["imports"] = serde_json::Value::Array(rust_crate_requests(modules));
        }
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }
    let modules = collect_modules_detailed_with_session(unit.source.clone(), &session)
        .map_err(|failure| failure.render_human())?;
    println!("{}", serde_json::to_string(&rust_crate_requests(modules))?);
    Ok(())
}

/// Each Rust crate request the collected modules import, with its version, features and the module it came from.
fn rust_crate_requests(modules: Vec<ParsedModule>) -> Vec<serde_json::Value> {
    let mut imports = Vec::new();
    for module in modules {
        for declaration in module.ast.declarations {
            let Declaration::Import(import) = declaration.node else {
                continue;
            };
            let (ImportKind::RustCrate {
                crate_name,
                version,
                features,
                ..
            }
            | ImportKind::RustFrom {
                crate_name,
                version,
                features,
                ..
            }) = import.kind
            else {
                continue;
            };
            imports.push(serde_json::json!({
                "crate": crate_name,
                "version": version,
                "features": features,
                "source": module.file_path,
            }));
        }
    }
    imports
}
