//! Export Rust import syntax from the compiler's ordinary source graph for Oven dependency preparation, and, with
//! `--source-graph`, what that graph's collection read and probed, which Oven keys a program or test unit by.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use incan_driver::modules::{
    SourceFileSurface, collect_modules_detailed_with_session, collect_source_graph_reads, source_file_surface,
};
use incan_driver::session::CompilationSession;
use incan_driver::unit::UnitDescription;
use incan_frontend::ast::{Declaration, ImportKind};
use incan_frontend::parsed_module::ParsedModule;
use sha2::{Digest, Sha256};

/// Print the source graph's Rust crate requests without checking Rust items or selecting artifacts. Oven resolves
/// these requests before checking; parsing uses the same module collection and dependency vocabulary as compilation.
///
/// `--rust-imports UNIT --source-graph` prints the files the collection read and the absent paths its imports looked
/// for instead, as `{"read": [...], "absent": [...], "complete": bool, "imports": [...], "surfaces": [...]}`, with the
/// same Rust crate requests when collection completes, so one request answers both questions. A collection the
/// compiler would refuse still prints what it read and probed (`complete` is false, no `imports` or `surfaces`): the
/// refusal depends on exactly those inputs. Each surface names one file read and the digest of what it contributed
/// to the collection (see [`surface_digest`]).
///
/// `--rust-imports UNIT --source-surfaces FILE...` collects only the named files of that graph, as
/// `{"surfaces": [...]}`: a recorded graph whose edited files kept their surfaces is still the unit's graph.
pub fn export(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (path, mode, files) = match arguments {
        [path] => (path, None, &[][..]),
        [path, flag] if flag == "--source-graph" => (path, Some(flag), &[][..]),
        [path, flag, files @ ..] if flag == "--source-surfaces" && !files.is_empty() => (path, Some(flag), files),
        _ => {
            return Err(
                "usage: incan-rustc-driver --rust-imports UNIT [--source-graph | --source-surfaces FILE...]".into(),
            );
        }
    };
    let unit = UnitDescription::read(Path::new(path)).map_err(|error| error.message)?;
    let session = CompilationSession::for_formatting(&unit).map_err(|error| error.message)?;
    match mode.map(String::as_str) {
        Some("--source-graph") => {
            let (reads, collected) = collect_source_graph_reads(unit.source.clone(), &session);
            let mut report = serde_json::json!({
                "read": reads.read,
                "absent": reads.absent,
                "complete": collected.is_ok(),
            });
            if let Ok(modules) = collected {
                report["surfaces"] = serde_json::Value::Array(collected_surfaces(&reads.surfaces, &modules));
                report["imports"] = serde_json::Value::Array(rust_crate_requests(&modules));
            }
            println!("{}", serde_json::to_string(&report)?);
        }
        Some(_) => {
            let mut surfaces = Vec::with_capacity(files.len());
            for file in files {
                let (surface, module) =
                    source_file_surface(PathBuf::from(file), &session).map_err(|failure| failure.render_human())?;
                surfaces.push(surface_entry(&surface, &module));
            }
            println!("{}", serde_json::to_string(&serde_json::json!({"surfaces": surfaces}))?);
        }
        None => {
            let modules = collect_modules_detailed_with_session(unit.source.clone(), &session)
                .map_err(|failure| failure.render_human())?;
            println!("{}", serde_json::to_string(&rust_crate_requests(&modules))?);
        }
    }
    Ok(())
}

/// Each collected file's surface entry, in reading order.
fn collected_surfaces(surfaces: &[SourceFileSurface], modules: &[ParsedModule]) -> Vec<serde_json::Value> {
    let by_path: HashMap<&PathBuf, &ParsedModule> = modules.iter().map(|module| (&module.file_path, module)).collect();
    surfaces
        .iter()
        .filter_map(|surface| by_path.get(&surface.path).map(|module| surface_entry(surface, module)))
        .collect()
}

/// One file's surface entry: its path and [`surface_digest`].
fn surface_entry(surface: &SourceFileSurface, module: &ParsedModule) -> serde_json::Value {
    serde_json::json!({"path": surface.path, "surface": surface_digest(surface, module)})
}

/// The digest of what one file contributes to its graph's collection: the module files its imports resolve to, the
/// absent paths their lookups probed, and its Rust crate requests. A graph whose edited files keep their digests is
/// collected the same.
fn surface_digest(surface: &SourceFileSurface, module: &ParsedModule) -> String {
    let document = serde_json::json!({
        "modules": surface.modules,
        "absent": surface.absent,
        "imports": module_crate_requests(module),
    });
    format!("{:x}", Sha256::digest(document.to_string().as_bytes()))
}

/// Each Rust crate request the collected modules import, with its version, features and the module it came from.
fn rust_crate_requests(modules: &[ParsedModule]) -> Vec<serde_json::Value> {
    modules.iter().flat_map(module_crate_requests).collect()
}

/// Each Rust crate request one module imports, in declaration order.
fn module_crate_requests(module: &ParsedModule) -> Vec<serde_json::Value> {
    let mut imports = Vec::new();
    for declaration in &module.ast.declarations {
        let Declaration::Import(import) = &declaration.node else {
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
        }) = &import.kind
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
    imports
}
