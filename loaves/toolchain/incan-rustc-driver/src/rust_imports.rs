//! Export Rust import syntax from the compiler's ordinary source graph for Oven dependency preparation.

use std::path::Path;

use incan_driver::modules::collect_modules_detailed_with_session;
use incan_driver::session::CompilationSession;
use incan_driver::unit::UnitDescription;
use incan_frontend::ast::{Declaration, ImportKind};
use incan_frontend::parsed_module::ParsedModule;

/// Parsed source retained across Oven's authoritative Rust artifact selection.
pub(crate) struct ImportActivation {
    provisional: UnitDescription,
    modules: Vec<ParsedModule>,
}

impl ImportActivation {
    /// Encode the typed crate, version and feature requirements without selecting any artifact.
    pub(crate) fn requirements(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&requirements(&self.modules))
    }

    /// Admit Oven's final description only when it preserves every source-semantic field parsed provisionally.
    pub(crate) fn finish(self, path: &Path) -> Result<Vec<ParsedModule>, Box<dyn std::error::Error>> {
        let final_unit = UnitDescription::read(path).map_err(|error| error.message)?;
        if !same_source_envelope(&self.provisional, &final_unit) {
            return Err("final unit description changed the staged source activation".into());
        }
        Ok(self.modules)
    }
}

/// Print the source graph's Rust crate requests without checking Rust items or selecting artifacts. Oven resolves
/// these requests before checking; parsing uses the same module collection and dependency vocabulary as compilation.
pub fn export(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [path] = arguments else {
        return Err("usage: incan-rustc-driver --rust-imports UNIT".into());
    };
    let activation = prepare(Path::new(path))?;
    println!("{}", String::from_utf8(activation.requirements()?)?);
    Ok(())
}

/// Parse one provisional description's source graph for a retained staged worker activation.
pub(crate) fn prepare(path: &Path) -> Result<ImportActivation, Box<dyn std::error::Error>> {
    let unit = UnitDescription::read(path).map_err(|error| error.message)?;
    let session = CompilationSession::for_formatting(&unit).map_err(|error| error.message)?;
    let modules = collect_modules_detailed_with_session(unit.source.clone(), &session)
        .map_err(|failure| failure.render_human())?;
    Ok(ImportActivation {
        provisional: unit,
        modules,
    })
}

/// Project parsed imports into requirements; selecting concrete registry artifacts remains Oven's authority.
fn requirements(modules: &[ParsedModule]) -> Vec<serde_json::Value> {
    let mut imports = Vec::new();
    for module in modules {
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
                "source": &module.file_path,
            }));
        }
    }
    imports
}

/// Source parsing may not be reused when Oven changes anything that governs collection, syntax or declaration identity.
fn same_source_envelope(provisional: &UnitDescription, final_unit: &UnitDescription) -> bool {
    provisional.source == final_unit.source
        && provisional.kind == final_unit.kind
        && provisional.program_entries == final_unit.program_entries
        && provisional.project_root == final_unit.project_root
        && provisional.source_root == final_unit.source_root
        && provisional.package == final_unit.package
        && provisional.registry_id == final_unit.registry_id
        && provisional.version == final_unit.version
        && provisional.profile == final_unit.profile
        && provisional.declared_features == final_unit.declared_features
        && provisional.active_features == final_unit.active_features
        && provisional.loaves == final_unit.loaves
        && provisional.contract_models == final_unit.contract_models
        && provisional.c_headers == final_unit.c_headers
        && provisional.vocab == final_unit.vocab
        && provisional.vocab_entry == final_unit.vocab_entry
        && provisional.vocabulary == final_unit.vocabulary
}
