//! Export Rust import syntax from the compiler's ordinary source graph for Oven dependency preparation.

use std::path::Path;

use incan_driver::modules::collect_modules_detailed_with_session;
use incan_driver::session::CompilationSession;
use incan_driver::unit::UnitDescription;
use incan_frontend::ast::{Declaration, ImportKind};

/// Print the source graph's Rust crate requests without checking Rust items or selecting artifacts. Oven resolves
/// these requests before checking; parsing uses the same module collection and dependency vocabulary as compilation.
pub fn export(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [path] = arguments else {
        return Err("usage: incan-rustc-driver --rust-imports UNIT".into());
    };
    let unit = UnitDescription::read(Path::new(path)).map_err(|error| error.message)?;
    let session = CompilationSession::for_formatting(&unit).map_err(|error| error.message)?;
    let modules = collect_modules_detailed_with_session(unit.source.clone(), &session)
        .map_err(|failure| failure.render_human())?;
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
    println!("{}", serde_json::to_string(&imports)?);
    Ok(())
}
